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
| 0.6.0 | A developer wires semlith into whichever agent they use — Claude Code, Codex, Copilot, Cursor, Zed, Gemini CLI, JetBrains — from a documented config for that client, gets a tool surface that covers what the CLI does rather than a fifth of it, and can read exactly which parts of semlith are covered by SemVer and which are not. | specified | released |
| 0.7.0 | A developer points semlith at a corpus far larger than one repository — a notes archive, a monorepo, several repositories at once. The first index of it says where it is and roughly what is left, and surviving an interruption costs the run its last few minutes rather than all of them. Searching that store costs a bounded amount of memory instead of an amount that grows with the corpus. | specified | released |
| 0.8.0 | A developer hands an agent a corpus of mixed formats — Word, PowerPoint and Excel documents, their OpenDocument equivalents, Jupyter notebooks, HTML pages — and gets the same ranked excerpts with locators back as from code and Markdown today, from the same index command, with no converter run by hand first. | specified | planned |


_Canonical sources: products/<id>/roadmap.yaml_
