"""granite-embedding-small-english-r2 (ModernBERT) re-implemented for Core ML.

The HF module cannot be traced to a fixed-shape Core ML program as-is (unpadding,
flash-attention paths, dynamic masks), so the forward pass is written out here
against the HF weights. Two layouts:

- 'std': (B, S, C), Linear and LayerNorm as usual. Runs well on the GPU through
  Core ML; the Neural Engine rejects most of it.
- 'ane': Apple's Neural Engine recipe (B, C, 1, S) with 1x1 Conv2d for every
  projection and per-head einsum attention (ml-ane-transformers, "Deploying
  Transformers on the Apple Neural Engine", 2022). In this layout > 99 % of ops
  stay on the Neural Engine; in the std layout most fall back to the CPU.

Both take ids (B, S) int32 and kmask (B, S) additive (0 keep, NEG pad) and
return the un-normalised CLS vector (B, C). Masks are additive rather than
boolean because fp16 programs on the ANE have no cheap select.
"""
import torch
import torch.nn as nn
import torch.nn.functional as F
from transformers import AutoModel

# Additive mask for a padded key. -1e4, not -inf: -inf + anything is NaN in fp16
# softmax, and -1e4 already underflows exp() to zero.
NEG = -1e4


def rope_tables(S, D, theta):
    inv = 1.0 / (theta ** (torch.arange(0, D, 2, dtype=torch.float) / D))
    f = torch.outer(torch.arange(S, dtype=torch.float), inv)
    emb = torch.cat([f, f], -1)
    return emb.cos(), emb.sin()


def window_bias(S, half):
    # Sliding-window layers attend |q - k| <= half. Baked in as a constant bias
    # because S is fixed per model.
    i = torch.arange(S)
    d = (i[:, None] - i[None, :]).abs()
    return torch.where(d <= half, 0.0, NEG)  # [q, k]


class Granite(nn.Module):
    def __init__(self, hf_path, S, layout='std'):
        super().__init__()
        hf = AutoModel.from_pretrained(hf_path, dtype=torch.float32).eval()
        c = hf.config
        self.layout, self.S = layout, S
        self.C, self.H = c.hidden_size, c.num_attention_heads
        self.D = self.C // self.H
        self.I = c.intermediate_size
        self.eps = c.norm_eps
        self.types = list(c.layer_types)
        self.emb = hf.embeddings.tok_embeddings
        self.emb_norm = hf.embeddings.norm.weight
        g = rope_tables(S, self.D, c.rope_parameters["full_attention"]["rope_theta"])
        l = rope_tables(S, self.D, c.rope_parameters["sliding_attention"]["rope_theta"])
        # transformers reports sliding_window as half the local_attention span (64).
        self.register_buffer('win', window_bias(S, c.sliding_window))
        self.register_buffer('gcos', g[0]); self.register_buffer('gsin', g[1])
        self.register_buffer('lcos', l[0]); self.register_buffer('lsin', l[1])
        self.layers = hf.layers
        self.final_norm = hf.final_norm.weight

    # ---------- std layout ----------
    def ln_std(self, x, w):
        return F.layer_norm(x, (self.C,), w, None, self.eps)

    def rot_std(self, x, cos, sin):  # x B,H,S,D
        x1, x2 = x[..., : self.D // 2], x[..., self.D // 2:]
        return x * cos + torch.cat([-x2, x1], -1) * sin

    def forward_std(self, ids, kmask):
        S = self.S
        x = self.ln_std(self.emb(ids), self.emb_norm)
        kb = kmask.unsqueeze(1).unsqueeze(1)
        for i, L in enumerate(self.layers):
            glob = self.types[i] == 'full_attention'
            cos, sin = (self.gcos, self.gsin) if glob else (self.lcos, self.lsin)
            # ModernBERT's first layer has no attention norm (the embedding norm serves).
            h = x if i == 0 else self.ln_std(x, L.attn_norm.weight)
            qkv = F.linear(h, L.attn.Wqkv.weight).reshape(-1, S, 3, self.H, self.D)
            q, k, v = [t.transpose(1, 2) for t in qkv.unbind(2)]
            q, k = self.rot_std(q, cos, sin), self.rot_std(k, cos, sin)
            a = (q @ k.transpose(-1, -2)) * (self.D ** -0.5) + kb
            if not glob:
                a = a + self.win
            o = (a.softmax(-1) @ v).transpose(1, 2).reshape(-1, S, self.C)
            x = x + F.linear(o, L.attn.Wo.weight)
            h = self.ln_std(x, L.mlp_norm.weight)
            u, gt = F.linear(h, L.mlp.Wi.weight).chunk(2, -1)
            x = x + F.linear(F.gelu(u) * gt, L.mlp.Wo.weight)
        x = self.ln_std(x, self.final_norm)
        return x[:, 0]

    # ---------- ANE layout ----------
    def ln_ane(self, x, w):  # x B,C,1,S; normalise over C
        # fp16-safe: square z/64, not z. z*z overflows fp16 once |z| > 256, and the
        # ANE then returns the whole row as zeros (the bench's zero rows on real
        # chunks). z/64 * rsqrt(mean((z/64)^2) + eps/64^2) == LN(z) exactly.
        m = x.mean(1, keepdim=True)
        z = (x - m) * (1.0 / 64)
        var = (z * z).mean(1, keepdim=True)
        return z * torch.rsqrt(var + self.eps / 4096) * w.view(1, -1, 1, 1)

    @staticmethod
    def conv(x, w):  # a Linear as a 1x1 conv: the ANE's native matmul
        return F.conv2d(x, w[:, :, None, None])

    def rot_ane(self, x, cos, sin):  # x B,D,1,S; cos (1,D,1,S)
        x1, x2 = x[:, : self.D // 2], x[:, self.D // 2:]
        return x * cos + torch.cat([-x2, x1], 1) * sin

    def forward_ane(self, ids, kmask):
        S = self.S
        x = self.emb(ids).transpose(1, 2).unsqueeze(2)  # B,C,1,S
        x = self.ln_ane(x, self.emb_norm)
        # Attention weights are laid out B,Sk,1,Sq so softmax runs over dim 1 (keys).
        kb = kmask.unsqueeze(-1).unsqueeze(-1)
        wb = self.win.t().contiguous().view(1, S, 1, S)
        tabs = {}
        for name, (c, s) in {'g': (self.gcos, self.gsin), 'l': (self.lcos, self.lsin)}.items():
            tabs[name] = (c.t().contiguous().view(1, self.D, 1, S), s.t().contiguous().view(1, self.D, 1, S))
        scale = self.D ** -0.5
        for i, L in enumerate(self.layers):
            glob = self.types[i] == 'full_attention'
            cos, sin = tabs['g' if glob else 'l']
            h = x if i == 0 else self.ln_ane(x, L.attn_norm.weight)
            qkv = self.conv(h, L.attn.Wqkv.weight)
            q, k, v = qkv.split(self.C, 1)
            # One einsum per head, per Apple's recipe: it avoids the (B,H,S,D) reshapes
            # and transposes that the Neural Engine handles badly.
            outs = []
            for hq, hk, hv in zip(q.split(self.D, 1), k.split(self.D, 1), v.split(self.D, 1)):
                hq = self.rot_ane(hq, cos, sin) * scale  # scale q before the dot: keeps fp16 in range
                hk = self.rot_ane(hk, cos, sin)
                w = torch.einsum('bchq,bkhc->bkhq', hq, hk.transpose(1, 3)) + kb
                if not glob:
                    w = w + wb
                w = w.softmax(1)
                outs.append(torch.einsum('bkhq,bchk->bchq', w, hv))
            x = x + self.conv(torch.cat(outs, 1), L.attn.Wo.weight)
            h = self.ln_ane(x, L.mlp_norm.weight)
            u, gt = self.conv(h, L.mlp.Wi.weight).split(self.I, 1)
            x = x + self.conv(F.gelu(u) * gt, L.mlp.Wo.weight)
        x = self.ln_ane(x, self.final_norm)
        return x[:, :, 0, 0]

    def forward(self, ids, kmask):
        return self.forward_ane(ids, kmask) if self.layout == 'ane' else self.forward_std(ids, kmask)
