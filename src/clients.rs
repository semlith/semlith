//! The client setup stanzas, read out of `docs/clients.md` at compile time.
//!
//! The portal's Agents view shows a copy-ready stanza per client, and there is
//! exactly one way for that to stay true as flags change: it has to be the
//! same text `tests/clients.rs` executes. So `docs/clients.md` is the source,
//! embedded with `include_str!` and parsed with the same rules the test uses — a heading
//! per client, then its fenced blocks.
//!
//! A stanza retyped into Rust would be a second copy that looks right for
//! exactly as long as nobody edits one of them.

use std::sync::OnceLock;

const CLIENTS_DOC: &str = include_str!("../docs/clients.md");

/// The heading the stanzas live under.
const SECTION: &str = "### Setting it up in your client";

/// The heading the HTTP stanzas live under.
const HTTP_SECTION: &str = "### Connecting over HTTP";

/// What a stanza names where the agent key goes.
///
/// The variable rather than the key from 0.14.0: a configuration file that
/// names it keeps working across every rotation, and one that carries the key
/// itself goes stale the moment somebody rotates. The portal substitutes the
/// literal value into this only when the reader has pressed Reveal.
const KEY_PLACEHOLDER: &str = "${SEMLITH_AGENT_KEY}";

/// What a stanza names where the semlith binary goes.
///
/// A registration naming the bare word `semlith` only launches from a `PATH`
/// that happens to carry it, and the process that reads these files is very
/// often not a login shell: an editor started from a desktop icon, a launchd or
/// systemd agent, and a desktop app all inherit a `PATH` that never sourced a
/// profile and so has never heard of `~/.cargo/bin`. The client reports that
/// the server exited and says nothing about why, which reads as semlith being
/// broken. So every registration semlith writes names the absolute path of the
/// binary that wrote it.
///
/// A placeholder in the document rather than a path assembled in Rust, for the
/// same reason as `KEY_PLACEHOLDER` above: `docs/clients.md` is the one copy of
/// every stanza, and a path built here would be a second one that looks right
/// for exactly as long as nobody edits the other.
const BIN_PLACEHOLDER: &str = "${SEMLITH_BIN}";

/// The placeholder a `rules` fence carries in place of the rule block itself.
///
/// The block lives in `docs/skill/RULES.md`, which is what `semlith setup`
/// writes and what `doctor` prints for a client whose rules file semlith does
/// not write. Repeating it per client in `docs/clients.md` would be six copies
/// to keep in step.
const RULES_PLACEHOLDER: &str = "${SEMLITH_RULES}";

/// The always-on rule block, embedded once.
pub const RULES: &str = include_str!("../docs/skill/RULES.md");

/// The Agent Skill `semlith setup` installs, embedded once.
pub const SKILL: &str = include_str!("../docs/skill/SKILL.md");

/// The semlith-explorer research subagent `setup` writes for Claude Code
/// (4.8): the skill's routing table in a read-only agent Claude chooses over
/// `Explore` for code research in an indexed folder.
pub const EXPLORER: &str = include_str!("../docs/skill/semlith-explorer.md");

/// One pasteable block.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Stanza {
    /// The fence's language: `json`, `toml`, `sh`, `yaml`.
    pub format: String,
    pub text: String,
    /// The one `sh register` fence a client with a registration CLI carries:
    /// the command `semlith setup` runs for it, scope flag and all.
    ///
    /// It lives in the fence's info string rather than in a table here because
    /// a rendered fence hides everything after the language, so the fact stays
    /// in the document a human reads and in the one a program parses at once.
    /// A command retyped into Rust would look right for exactly as long as
    /// nobody edited one of the two.
    pub register: bool,
    /// From a bare `unregister` on a fence: a command that removes an existing
    /// semlith entry from this client, run before the registration.
    pub unregister: bool,
    /// From `path=` in the info string: the user-level configuration file
    /// `semlith setup --register-all` may merge into.
    ///
    /// `None` on every fence that is an example rather than a destination —
    /// which is every project-level file, because registering semlith into one
    /// repository's committed config is the defect 0.18.0 exists to end.
    pub path: Option<String>,
    /// From `os=` in the info string, when a client's file is somewhere else on
    /// another platform. `None` means the path is the same everywhere.
    pub os: Option<String>,
    /// From a bare `hook` on a fence: the `PreToolUse` block `semlith setup`
    /// merges into this client's settings, and `semlith setup --no-hooks`
    /// removes again. Its `path=` is the settings file.
    pub hook: bool,
    /// From a bare `skills` on a fence: its `path=` is a user-level skill
    /// directory this client reads, which `semlith setup` links the canonical
    /// skill into.
    pub skills: bool,
    /// From a bare `rules` on a fence: its `path=` is a user-level rules file,
    /// written only under `--register-all` because it is prose a person owns
    /// rather than a server list semlith put there.
    pub rules: bool,
    /// From `scope=` on a `register` fence: what that command actually
    /// registers.
    ///
    /// `global` unless the fence says otherwise, and two clients say otherwise.
    /// `opencode mcp add` on 1.18.11 and `kilo mcp add` have no global flag, so
    /// running them registers semlith for the directory the user happened to be
    /// standing in — which is the defect this release exists to end. semlith
    /// does not run a `scope=project` command; those clients reach every
    /// project through their user-level file instead.
    pub scope: Scope,
}

/// What a registration command registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Every project on the machine. The only kind semlith runs unasked.
    #[default]
    Global,
    /// The directory it is run in, and nothing else.
    Project,
}

/// What a fence's info string says beyond its language.
///
/// `sh register`, `json config path=~/.cursor/mcp.json`, and
/// `json config path=%APPDATA%\Claude\claude_desktop_config.json os=windows`
/// are the three shapes in use. An attribute this does not know is ignored
/// rather than refused: `docs/clients.md` is documentation first, and a fence
/// annotated for some future reader should not stop the portal rendering.
fn info(line: &str) -> Info {
    let mut format = String::new();
    let (mut register, mut unregister) = (false, false);
    let (mut hook, mut skills, mut rules) = (false, false, false);
    let (mut path, mut os, mut scope) = (None, None, Scope::Global);
    // Hand-split rather than `split_whitespace`, because one path has a space
    // in it — Claude Desktop's on macOS — and a quoted value has to survive.
    for word in split_attributes(line) {
        match word.split_once('=') {
            Some(("path", value)) => path = Some(unquote(value).to_string()),
            Some(("os", value)) => os = Some(unquote(value).to_string()),
            Some(("scope", "project")) => scope = Scope::Project,
            Some(("scope", _)) => scope = Scope::Global,
            _ if word == "register" => register = true,
            _ if word == "unregister" => unregister = true,
            _ if word == "hook" => hook = true,
            _ if word == "skills" => skills = true,
            _ if word == "rules" => rules = true,
            _ if format.is_empty() => format = word.to_string(),
            _ => {}
        }
    }
    Info {
        format,
        register,
        unregister,
        hook,
        skills,
        rules,
        path,
        os,
        scope,
    }
}

/// What an info string carried, beyond the fence's language.
struct Info {
    format: String,
    register: bool,
    unregister: bool,
    hook: bool,
    skills: bool,
    rules: bool,
    path: Option<String>,
    os: Option<String>,
    scope: Scope,
}

/// An info string's words, keeping a double-quoted value whole.
fn split_attributes(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                word.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(value)
}

/// One client, its note, and every stanza `docs/clients.md` gives for it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Client {
    pub name: String,
    /// Where the file lives and anything the client needs told about it, as
    /// one line of plain text.
    pub note: String,
    /// `terminal`, `editor` or `desktop`, read from `docs/clients.md`'s own grouping
    /// so the portal's tabs and the documentation cannot disagree about which
    /// kind of thing a client is.
    pub group: String,
    pub stanzas: Vec<Stanza>,
}

impl Client {
    /// The command `semlith setup` runs to register this client, or `None` for
    /// a client whose only route in is a file somebody edits.
    ///
    /// Whitespace-collapsed, because a fence may wrap a long command and a
    /// newline inside an argument list is not part of the command.
    pub fn register_command(&self) -> Option<String> {
        self.stanzas
            .iter()
            .find(|stanza| stanza.register)
            .map(|stanza| stanza.text.split_whitespace().collect::<Vec<_>>().join(" "))
    }

    /// The commands that remove an existing semlith entry from this client,
    /// run before the registration and with their status ignored.
    ///
    /// More than one where a client has more than one scope to clear: Claude
    /// Code's `user`, `project` and `local` are three separate entries, and the
    /// one that made this release necessary was at `local`. A client that
    /// documents no remove verb has none here, and semlith reports what the
    /// failing `add` said rather than guessing a command.
    pub fn unregister_commands(&self) -> Vec<String> {
        self.stanzas
            .iter()
            .filter(|stanza| stanza.unregister)
            .map(|stanza| stanza.text.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect()
    }

    /// Whether that command registers semlith for every project.
    ///
    /// The only kind `semlith setup` runs unasked. A command that registers the
    /// current directory would leave the user exactly where 0.18.0 found
    /// them — semlith working in the repository they happened to be standing in
    /// and missing from the next one — so those clients go through their
    /// user-level file instead.
    pub fn registers_globally(&self) -> bool {
        self.stanzas
            .iter()
            .any(|stanza| stanza.register && stanza.scope == Scope::Global)
    }

    /// The user-level configuration files for this client.
    ///
    /// `semlith doctor` reads them to report where a client actually registered
    /// and at what scope. `semlith setup --register-all` writes them, but only
    /// for a client with no global registration CLI: a client semlith can ask
    /// to register itself is not a file semlith writes.
    pub fn config_files(&self) -> impl Iterator<Item = &Stanza> {
        self.stanzas
            .iter()
            .filter(|stanza| stanza.path.is_some() && !stanza.hook && !stanza.rules)
    }

    /// The `PreToolUse` block for this client, if it documents one.
    ///
    /// One per client: a second would be two hooks doing the same job, and the
    /// removal path would have to guess which one semlith put there.
    pub fn hook_stanza(&self) -> Option<&Stanza> {
        self.stanzas
            .iter()
            .find(|stanza| stanza.hook && stanza.path.is_some())
    }

    /// The user-level skill directories this client reads.
    pub fn skill_dirs(&self) -> impl Iterator<Item = &Stanza> {
        self.stanzas
            .iter()
            .filter(|stanza| stanza.skills && stanza.path.is_some())
    }

    /// The user-level rules file this client reads, if semlith knows one.
    ///
    /// A client without one is not a client semlith writes prose into on a
    /// guess: `doctor` prints the block for a person to paste instead.
    pub fn rules_file(&self) -> Option<&Stanza> {
        self.stanzas
            .iter()
            .find(|stanza| stanza.rules && stanza.path.is_some())
    }

    /// Whether `semlith setup --register-all` would write this client's file.
    pub fn needs_a_file_written(&self) -> bool {
        !self.registers_globally() && self.config_files().next().is_some()
    }
}

/// The clients `docs/clients.md` documents no way for semlith to register.
///
/// Not an omission and not a to-do. Crush and Roo Code document only a
/// project-level file, and registering semlith into one repository's committed
/// configuration is the defect this release exists to end rather than a smaller
/// version of the fix. Zed's own instruction is to open its settings through a
/// command palette entry, and no vendor documentation gives the path that
/// opens. `semlith doctor` names these three and prints their stanza rather
/// than reporting them as failures, because nothing is broken: there is
/// nowhere to write.
pub const UNREGISTERABLE: [&str; 3] = ["Crush", "Zed", "Roo Code"];

/// The HTTP stanzas, with `key` substituted for `docs/clients.md`'s placeholder.
///
/// One template for every client rather than one per client: the endpoint and
/// the header are the same wherever they are pasted, and a per-client copy
/// would be twelve places for one URL to go stale.
pub fn http_stanzas(key: &str) -> Vec<Stanza> {
    static PARSED: OnceLock<Vec<Stanza>> = OnceLock::new();
    PARSED
        .get_or_init(|| parse_http(CLIENTS_DOC))
        .iter()
        .map(|stanza| Stanza {
            text: stanza.text.replace(KEY_PLACEHOLDER, key),
            ..stanza.clone()
        })
        .collect()
}

fn parse_http(readme: &str) -> Vec<Stanza> {
    let Some(start) = readme.find(HTTP_SECTION) else {
        return Vec::new();
    };
    let rest = &readme[start + HTTP_SECTION.len()..];
    let section = match rest.find("\n## ") {
        Some(end) => &rest[..end],
        None => rest,
    };
    let mut out = Vec::new();
    let mut lines = section.lines();
    while let Some(line) = lines.next() {
        let Some(format) = line.strip_prefix("```") else {
            continue;
        };
        let mut text = String::new();
        for body in lines.by_ref() {
            if body.starts_with("```") {
                break;
            }
            text.push_str(body);
            text.push('\n');
        }
        out.push(stanza(info(format), &text));
    }
    out
}

/// The absolute path of the binary this process is running from, resolved once.
///
/// Every stanza goes through here on its way out of `docs/clients.md`, so a
/// caller cannot forget it and there is no second spelling of the path for a
/// client to disagree with — which also makes registering twice write the same
/// bytes twice.
pub fn binary_path() -> &'static str {
    static RESOLVED: OnceLock<String> = OnceLock::new();
    RESOLVED.get_or_init(resolve_binary_path)
}

/// The running binary, spelled with forward slashes on every platform.
///
/// Forward slashes deliberately, and not only for tidiness. `canonicalize` on
/// Windows returns the verbatim form — `\\?\C:\Users\you\.cargo\bin\semlith.exe`
/// — which several client launchers refuse to execute, and whose backslashes
/// would each have to be doubled to survive being embedded in the JSON, TOML
/// and double-quoted YAML these stanzas are written in. Miss one of those
/// escapes and the client's parser rejects the whole file, taking every other
/// server in it with semlith. `C:/Users/you/.cargo/bin/semlith.exe` is accepted
/// by `CreateProcess`, needs no escaping in any of the four formats, and is the
/// one spelling in all of them. On unix the separator is already `/` and the
/// replacement does nothing.
///
/// The bare command when `current_exe` fails, rather than a guess: it can fail
/// on a platform with no `/proc` and a binary that has been unlinked, and a
/// stanza naming `semlith` at least works wherever the `PATH` is right — which
/// is where this release found everybody. A wrong absolute path works nowhere,
/// and unlike the bare word it gives the reader nothing to recognise.
fn resolve_binary_path() -> String {
    let Ok(running) = std::env::current_exe() else {
        return "semlith".to_string();
    };
    // `unwrap_or`: `current_exe` already hands back an absolute path, and
    // canonicalising it only resolves the symlinks a package manager leaves
    // behind. If that fails the absolute path is still worth writing.
    let resolved = std::fs::canonicalize(&running).unwrap_or(running);
    let text = resolved.to_string_lossy().into_owned();
    if !cfg!(windows) {
        return text;
    }
    // `\\?\UNC\server\share\…` is the verbatim spelling of `\\server\share\…`,
    // and stripping only the `\\?\` off it would leave a path beginning with a
    // literal `UNC` that names nothing.
    match text.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => text.strip_prefix(r"\\?\").unwrap_or(&text).to_string(),
    }
    .replace('\\', "/")
}

/// One parsed fence, with the binary path substituted for the placeholder
/// `docs/clients.md` writes.
fn stanza(info: Info, text: &str) -> Stanza {
    Stanza {
        format: info.format,
        text: text
            .replace(BIN_PLACEHOLDER, binary_path())
            .replace(RULES_PLACEHOLDER, RULES.trim_end()),
        register: info.register,
        unregister: info.unregister,
        hook: info.hook,
        skills: info.skills,
        rules: info.rules,
        path: info.path,
        os: info.os,
        scope: info.scope,
    }
}

/// Every documented client, parsed once.
pub fn clients() -> &'static [Client] {
    static PARSED: OnceLock<Vec<Client>> = OnceLock::new();
    PARSED.get_or_init(|| parse(CLIENTS_DOC))
}

fn parse(readme: &str) -> Vec<Client> {
    let Some(start) = readme.find(SECTION) else {
        return Vec::new();
    };
    let rest = &readme[start + SECTION.len()..];
    // The section ends at the next heading of its own level or above. Stopping
    // only at `## ` swept in the `### Connecting over HTTP` section below it,
    // and its example stanzas were then read as the last client's own.
    let end = ["\n## ", "\n### "]
        .iter()
        .filter_map(|marker| rest.find(marker))
        .min();
    let section = match end {
        Some(end) => &rest[..end],
        None => rest,
    };

    let mut out: Vec<Client> = Vec::new();
    let mut group = String::from("terminal");
    let mut lines = section.lines().peekable();
    while let Some(line) = lines.next() {
        // `docs/clients.md` groups its clients under `#### Terminal`, `#### Editors`
        // and `#### Desktop apps`. Read from there rather than from a table in
        // this file, so the grouping has one source.
        if let Some(heading) = line.strip_prefix("#### ") {
            group = match heading.trim() {
                "Editors" => "editor",
                "Desktop apps" => "desktop",
                _ => "terminal",
            }
            .to_string();
            continue;
        }
        // A client's block starts at its bold name at the start of a line.
        let Some(after) = line.strip_prefix("**") else {
            continue;
        };
        let Some((name, tail)) = after.split_once("**") else {
            continue;
        };

        // The note runs to the first blank line, and is joined into one line:
        // the portal shows it under the client's name, not as a paragraph.
        let mut note = tail.trim_start_matches([' ', '—', '-']).trim().to_string();
        for next in lines.by_ref() {
            if next.trim().is_empty() {
                break;
            }
            if next.starts_with("```") || next.starts_with("**") {
                break;
            }
            note.push(' ');
            note.push_str(next.trim());
        }

        let mut stanzas = Vec::new();
        // Then every fenced block until the next client or the end.
        while let Some(peeked) = lines.peek() {
            // The next client, or the heading of the next group — either way
            // this client's blocks have ended.
            if peeked.starts_with("**") || peeked.starts_with("#### ") {
                break;
            }
            let line = lines.next().expect("just peeked");
            let Some(format) = line.strip_prefix("```") else {
                continue;
            };
            let mut text = String::new();
            for body in lines.by_ref() {
                if body.starts_with("```") {
                    break;
                }
                text.push_str(body);
                text.push('\n');
            }
            stanzas.push(stanza(info(format), &text));
        }

        if !stanzas.is_empty() {
            out.push(Client {
                name: name.trim().to_string(),
                note: squeeze(&note),
                group: group.clone(),
                stanzas,
            });
        }
    }
    out
}

/// Markdown emphasis and backticks are noise in a portal panel that is not
/// rendering markdown.
fn squeeze(note: &str) -> String {
    note.replace("**", "")
        .replace('`', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// If this ever comes back empty, the Agents view silently shows nothing
    /// and `docs/clients.md` has been restructured under it.
    #[test]
    fn every_documented_client_is_parsed_with_at_least_one_stanza() {
        let parsed = clients();
        assert!(
            parsed.len() >= 27,
            "only {} clients parsed out of the CLIENTS_DOC",
            parsed.len()
        );
        for client in parsed {
            assert!(!client.name.is_empty());
            assert!(!client.stanzas.is_empty(), "{} has no stanza", client.name);
            for stanza in &client.stanzas {
                assert!(
                    !stanza.text.trim().is_empty(),
                    "{} has an empty stanza",
                    client.name
                );
            }
        }
    }

    /// The release's promise, asserted against the text the portal hands out:
    /// no client stanza carries a store path.
    #[test]
    fn no_parsed_stanza_carries_a_store_path() {
        for client in clients() {
            for stanza in &client.stanzas {
                assert!(
                    !stanza.text.contains("--store"),
                    "{} still passes --store:\n{}",
                    client.name,
                    stanza.text
                );
            }
        }
    }

    /// Every client is in one of the three groups the portal renders as tabs,
    /// and the counts sum to the number of clients `docs/clients.md` documents.
    #[test]
    fn every_client_carries_a_group_read_from_the_readme() {
        let parsed = clients();
        let mut counts = std::collections::BTreeMap::new();
        for client in parsed {
            assert!(
                ["terminal", "editor", "desktop"].contains(&client.group.as_str()),
                "{} is in group {:?}",
                client.name,
                client.group
            );
            *counts.entry(client.group.as_str()).or_insert(0usize) += 1;
        }
        assert_eq!(
            counts.values().sum::<usize>(),
            parsed.len(),
            "the groups do not cover every client"
        );
        assert_eq!(counts.len(), 3, "a group is empty: {counts:?}");
    }

    /// The HTTP stanzas carry the live key rather than the placeholder, and
    /// name the endpoint the daemon actually serves.
    #[test]
    fn the_http_stanzas_carry_the_key_they_are_given() {
        let stanzas = http_stanzas("sml_abc123");
        assert!(!stanzas.is_empty(), "no HTTP stanza was parsed");
        for stanza in &stanzas {
            assert!(
                stanza.text.contains("127.0.0.1:7365/mcp"),
                "an HTTP stanza does not name the endpoint:\n{}",
                stanza.text
            );
            assert!(
                stanza.text.contains("Bearer sml_abc123"),
                "an HTTP stanza does not carry the key it was given:\n{}",
                stanza.text
            );
            assert!(
                !stanza.text.contains(KEY_PLACEHOLDER),
                "an HTTP stanza still carries the placeholder:\n{}",
                stanza.text
            );
        }
    }

    /// Sixteen of the twenty-seven clients have a registration CLI, and those
    /// sixteen are exactly the set `semlith setup` registers without being
    /// asked. The number is asserted rather than counted at runtime because a
    /// client silently losing its `sh register` fence is a client that goes
    /// back to being a stanza somebody pastes, which is the defect 0.18.0
    /// exists to end and which nothing else here would notice.
    #[test]
    fn sixteen_clients_carry_a_registration_command() {
        let with: Vec<&str> = clients()
            .iter()
            .filter(|client| client.register_command().is_some())
            .map(|client| client.name.as_str())
            .collect();
        assert_eq!(with.len(), 16, "registration commands found: {with:?}");
    }

    /// Every documented client is reachable, and the three that are not are
    /// named rather than left to be discovered.
    ///
    /// A client is reachable when semlith can register it for every project:
    /// by its own CLI, or by writing its user-level configuration file under
    /// `--register-all`. The three in `UNREGISTERABLE` are neither, for the
    /// reasons recorded there. A fourth appearing here means a client lost its
    /// route in without anyone deciding that it should.
    #[test]
    fn every_client_is_reachable_or_is_one_of_the_three_that_are_not() {
        let mut unreachable = Vec::new();
        let mut by_file = Vec::new();
        for client in clients() {
            if client.registers_globally() {
                continue;
            }
            if client.needs_a_file_written() {
                by_file.push(client.name.as_str());
                continue;
            }
            unreachable.push(client.name.as_str());
        }
        assert_eq!(
            unreachable,
            UNREGISTERABLE.to_vec(),
            "the set semlith cannot register has changed; by file: {by_file:?}"
        );
    }

    /// Two clients register the directory they are run in, and semlith does not
    /// run those. Asserted by name: if a third joins them, or one of these two
    /// gains a global flag, that is a release decision rather than an edit.
    #[test]
    fn the_project_scoped_registrations_are_the_two_that_are_known_to_be() {
        let project: Vec<&str> = clients()
            .iter()
            .filter(|client| {
                client
                    .stanzas
                    .iter()
                    .any(|stanza| stanza.register && stanza.scope == Scope::Project)
            })
            .map(|client| client.name.as_str())
            .collect();
        assert_eq!(project, vec!["OpenCode", "Kilo Code"]);
        for name in &project {
            let client = clients()
                .iter()
                .find(|client| &client.name == name)
                .expect("just found");
            assert!(
                !client.registers_globally(),
                "{name} is counted as global as well as project-scoped"
            );
            assert!(
                client.needs_a_file_written(),
                "{name} has no global route in at all, which would make it unreachable"
            );
        }
    }

    /// A configuration file semlith writes is user-level. A project-level path
    /// here would have semlith registering itself into one repository's
    /// committed config, which is the scope bug wearing different clothes.
    #[test]
    fn every_config_path_semlith_writes_is_user_level() {
        for client in clients() {
            for stanza in client.config_files() {
                let path = stanza.path.as_deref().expect("filtered on Some");
                assert!(
                    path.starts_with('~') || path.starts_with('%') || path.starts_with('/'),
                    "{}'s config path {path} is not user-level",
                    client.name
                );
            }
        }
    }

    /// A registration semlith performs never carries a credential and never
    /// names the endpoint: it launches `semlith mcp`, which reads the key out
    /// of `~/.semlith/agent.key` itself. This is the whole of what 0.18.0
    /// changed about how a client authenticates, so it is asserted over the
    /// text rather than left to the shape of the command.
    #[test]
    fn no_registration_command_carries_a_key_or_an_endpoint() {
        for client in clients() {
            let Some(command) = client.register_command() else {
                continue;
            };
            for forbidden in [KEY_PLACEHOLDER, "Bearer", "127.0.0.1:7365", "sml_"] {
                assert!(
                    !command.contains(forbidden),
                    "{}'s registration command carries {forbidden}:\n{command}",
                    client.name
                );
            }
            assert!(
                command.contains("semlith"),
                "{}'s registration command does not name semlith:\n{command}",
                client.name
            );
        }
    }

    #[test]
    fn claude_code_is_the_first_client_and_has_both_of_its_forms() {
        let first = &clients()[0];
        assert_eq!(first.name, "Claude Code");
        // Asserted by role rather than by position. The list grew an
        // `unregister` fence in 0.18.0 and will grow again; what has to stay
        // true is that Claude Code documents the endpoint as a file and as a
        // command, and carries exactly one registration.
        assert!(
            first
                .stanzas
                .iter()
                .all(|s| ["json", "sh", "text"].contains(&s.format.as_str())),
            "an unexpected fence language on Claude Code: {:?}",
            first.stanzas.iter().map(|s| &s.format).collect::<Vec<_>>()
        );
        assert_eq!(
            first.stanzas.iter().filter(|s| s.register).count(),
            1,
            "a client may document exactly one registration command"
        );
        assert!(
            !first.unregister_commands().is_empty(),
            "the entry being replaced is often not at the scope the new one goes to"
        );
        assert!(
            first.stanzas[0].text.contains("http://127.0.0.1:7365/mcp"),
            "the first stanza is not the endpoint:\n{}",
            first.stanzas[0].text
        );
        // The stdio form, at user scope, is what `semlith setup` runs from
        // 0.18.0. The scope flag is the release: without it this command
        // registers the directory it was typed in. The path is this test
        // binary's own, because `binary_path` resolves whatever is running.
        assert_eq!(
            first.register_command().as_deref(),
            Some(
                format!(
                    "claude mcp add --scope user semlith -- \"{}\" mcp",
                    binary_path()
                )
                .as_str()
            )
        );
        assert!(first.registers_globally());
    }

    /// Every command `semlith setup` runs hands the client the binary's
    /// absolute path, in one piece.
    ///
    /// Asserted through `setup::argv`, which is what actually splits these
    /// commands, rather than over the text: the path has to be quoted in the
    /// document or a machine whose home has a space in it — `C:/Users/Ada
    /// Lovelace/…`, and every Windows machine with a full name on it — would
    /// register a program called `C:/Users/Ada` and an argument called
    /// `Lovelace/…`. Word-containment rather than equality because one client,
    /// Droid, takes the whole command line as a single quoted argument.
    #[test]
    fn every_registration_hands_the_client_the_resolved_path_whole() {
        for client in clients() {
            let Some(command) = client.register_command() else {
                continue;
            };
            let (_, args) = crate::setup::argv(&command).unwrap_or_else(|| {
                panic!("{}'s registration does not split: {command}", client.name)
            });
            assert!(
                args.iter().any(|word| word.contains(binary_path())),
                "{}'s registration does not hand over {} in one piece:\n{command}",
                client.name,
                binary_path()
            );
        }
    }

    /// The resolved path is absolute and carries no `\\?\` prefix.
    ///
    /// A verbatim path is what `canonicalize` hands back on Windows, and one
    /// written into a client's configuration is a defect: several launchers
    /// refuse to execute it, and its backslashes have to be escaped again for
    /// every format these stanzas are written in.
    #[test]
    fn the_resolved_binary_path_is_absolute_and_not_verbatim() {
        let path = binary_path();
        assert!(
            // `C:/Users/…` on Windows, where the drive letter is what makes it
            // absolute and there is no leading separator.
            path.starts_with('/') || path.contains(":/"),
            "the resolved binary path is not absolute: {path}"
        );
        assert!(
            !path.contains('\\'),
            "the resolved binary path is in the verbatim or backslash \
             spelling, which no stanza can embed unescaped: {path}"
        );
    }
}
