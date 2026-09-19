# docs/skill

The source of the Agent Skill semlith ships, kept here so it is reviewed with
the code it describes rather than drifting in a separate repository.

`SKILL.md` is the skill itself, in agentskills.io format. `semlith setup`
installs it as the canonical copy at `~/.semlith/skills/semlith/SKILL.md` and
links that directory into the user-level skill directories documented in
[../clients.md](../clients.md), so one file is installed once and every client
that supports skills reads the same text. Re-running `setup` refreshes it, which
makes it the repair command for a skill as much as for a registration.

`RULES.md` is the always-on rule block for clients that have no skill mechanism,
only a rules file — AGENTS.md, Kiro steering, Windsurf, Cline. It stands alone
without the skill, and it is written into those files only under
`semlith setup --register-all`, because editing a file an agent already owns is
not something a plain `setup` should do unasked.
