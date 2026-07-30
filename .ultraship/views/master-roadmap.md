<!--
Generated from canonical UltraShip state. Do not edit directly.
Run `ultraship views` to regenerate.
-->
# Master roadmap

## semlith

| Version | Outcome | Detail | Status |
| --- | --- | --- | --- |
| 0.1.0 | A developer indexes their docs and code locally and gets ranked excerpts with file:line locators back, from the CLI or from an agent over MCP. | outline | released |
| 0.2.0 | A developer indexes a repository on an ordinary laptop without watching memory or fearing a second terminal, and one search finds both an exact identifier and a plain-English question. | specified | released |
| 0.3.0 | A developer narrows a search to part of a corpus by path, extension or language, so an agent can ask about one subsystem instead of everything. | specified | released |
| 0.4.0 | A developer leaves a store current without re-running index by hand; edited files are re-embedded as they are saved, and an agent already connected over MCP searches the new contents without restarting its server. | specified | released |
| 0.5.0 | A developer searches several stores at once from a single query, so an agent working across repositories asks one question instead of many. | specified | released |
| 0.6.0 | A developer wires semlith into whichever agent they use — Claude Code, Codex, Copilot, Cursor, Zed, Gemini CLI, JetBrains — from a documented config for that client, gets a tool surface that covers what the CLI does rather than a fifth of it, and can read exactly which parts of semlith are covered by SemVer and which are not. | specified | planned |
| 0.7.0 | A developer runs one semlith against a corpus far larger than a repository — the store is searched without loading every vector into memory, and a first index of it is resumable. | hypothesis | planned |
| 1.0.0 | A developer pins semlith in a script or an agent config and the contract 0.6.0 wrote down stops being provisional: a break needs a major version. | hypothesis | planned |


_Canonical sources: products/<id>/roadmap.yaml_
