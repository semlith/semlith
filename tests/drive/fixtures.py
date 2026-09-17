"""The corpora the drive needs, built from nothing each run.

The manual drive that produced the findings document ran against one
developer's machine: a `semlith-drive-corpus` directory, six stores that
happened to be open, an `ultraship` checkout next door. None of that is a
gate. This module builds the same shapes from scratch so the drive asserts
against a corpus it created and can reason about.

Everything is built lazily. A `--only 4.13` run should not spend four minutes
generating six hundred files it will never index, so each corpus is a method
that builds on first call and caches afterwards.

The binary under test comes from `SEMLITH_BIN`, defaulting to `semlith` on
PATH. Where a fixture needs a store built on disk — the adopt case — it is
built by shelling out to that binary, so the store on disk is whatever the
release under test writes rather than whatever this file thinks a store
looks like.
"""

import json
import os
import shutil
import subprocess
import tempfile


class FixtureError(Exception):
    """A corpus could not be built, which is the harness's fault, not the product's."""


def semlith_bin():
    return os.environ.get("SEMLITH_BIN", "semlith")


def corpus_root():
    """Where the corpora are built, somewhere the portal's pickers can reach.

    `/api/dirs` and `/api/projects` refuse any path outside the user's home
    directory, deliberately: they are a browser asking a local server to list a
    filesystem. A corpus in the system temp directory is therefore invisible to
    every check that drives a picker, and on macOS the system temp directory is
    under `/var/folders`, which is never under `$HOME`.

    So the system temp directory is used when it is inside the home — which is
    what the README's advice to point `TMPDIR` there achieves — and the home
    itself otherwise. Either way the corpus is removed on the way out.
    """
    home = os.path.realpath(os.path.expanduser("~"))
    system = os.path.realpath(tempfile.gettempdir())
    if system == home or system.startswith(home + os.sep):
        return system
    return home


def run(args, cwd=None, timeout=600):
    """Run a command, returning its combined output, raising on failure."""
    try:
        finished = subprocess.run(
            args,
            cwd=cwd,
            timeout=timeout,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
    except FileNotFoundError:
        raise FixtureError(
            "%r is not on PATH. Set SEMLITH_BIN to the binary under test." % args[0]
        )
    except subprocess.TimeoutExpired:
        raise FixtureError("%r did not finish within %ds" % (" ".join(args), timeout))
    output = finished.stdout.decode("utf-8", "replace")
    if finished.returncode != 0:
        raise FixtureError(
            "%r exited %d:\n%s" % (" ".join(args), finished.returncode, output)
        )
    return output


# A small, deliberately varied source file. Varied because the Files table
# columns the drive asserts on — READ AS, LANGUAGE, LINES — are derived from
# the extension and the content.
SOURCE = """\
// %(name)s — a file the semlith drive generated.
//
// It exists so the store under test holds something with a shape: a symbol to
// find in the graph, a few lines to count, and a word a search can match.

pub struct %(symbol)s {
    pub name: String,
    pub count: usize,
}

impl %(symbol)s {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_string(), count: %(index)d }
    }

    /// Releases whatever this holds, so the next run can acquire it.
    pub fn release(&mut self) {
        self.count = 0;
    }

    /// One line far wider than any pane, so a check about a preview that
    /// scrolls has something that actually scrolls. Finding 4.3.
    pub fn describe_at_length(&self) -> String {
        format!("{} holds {} and releases it when the run that acquired it has finished with it, which is the whole of what this line is for", self.name, self.count)
    }
}
"""

DOC = """\
# %(name)s

A document the semlith drive generated, so the corpus is not all one language
and the `prefer docs` weighting has something to prefer.

The release record is sealed and immutable once the gates are green.
"""




class Fixtures:
    """Every corpus the checks can ask for, under one temp directory."""

    def __init__(self, keep=False):
        self.root = tempfile.mkdtemp(prefix="semlith-drive-corpus-", dir=corpus_root())
        self.keep = keep
        self._adoptme = None
        self._adopted = 0
        self._monorepo = None
        self._bulk = None
        self._small = None
        self._second = None
        self._unique = 0
        self._doomed = None

    def cleanup(self):
        if self.keep:
            return
        # The corpora, and the stores the drive made out of them. A drive that
        # left `small`, `bulk`, `doomed` and `adoptme` registered would leave
        # the next one choosing `small-2`, and the developer who ran it looking
        # at a store list that is not theirs.
        #
        # Found by root rather than by name, because the daemon picks the name
        # and picks a free one. Anything registered against a directory inside
        # this drive's own temp root is this drive's.
        for name in sorted(self._stores_under_root()):
            subprocess.run(
                [semlith_bin(), "drop", name, "--yes"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            )
        shutil.rmtree(self.root, ignore_errors=True)

    def _stores_under_root(self):
        """Every registered store whose roots lie inside this drive's corpora."""
        home = os.environ.get("SEMLITH_HOME") or os.path.join(
            os.path.expanduser("~"), ".semlith"
        )
        try:
            with open(os.path.join(home, "registry.json"), encoding="utf-8") as handle:
                registry = json.load(handle)
        except (OSError, ValueError):
            return []
        mine = os.path.realpath(self.root)
        out = []
        for name, entry in (registry.get("stores") or {}).items():
            roots = [os.path.realpath(root) for root in entry.get("roots") or []]
            if roots and all(root.startswith(mine) for root in roots):
                out.append(name)
        return out

    # ---------------------------------------------------------------- pieces

    def _write_corpus(self, directory, count, prefix="file"):
        os.makedirs(directory, exist_ok=True)
        for index in range(count):
            name = "%s_%03d" % (prefix, index)
            if index % 10 == 9:
                path = os.path.join(directory, name + ".md")
                body = DOC % {"name": name}
            else:
                path = os.path.join(directory, name + ".rs")
                body = SOURCE % {
                    "name": name,
                    "symbol": "Widget%03d" % index,
                    "index": index,
                }
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(body)
        return directory

    def _git_repo(self, directory):
        """A directory that `home::projects_under` will see as a project.

        It looks for git repositories first and falls back to plain
        subdirectories, so the picker fixture needs both kinds to prove the
        distinction is being made.
        """
        os.makedirs(directory, exist_ok=True)
        self._write_corpus(directory, 3, prefix=os.path.basename(directory))
        try:
            run(["git", "init", "--quiet"], cwd=directory)
            run(["git", "add", "-A"], cwd=directory)
            run(
                [
                    "git",
                    "-c",
                    "user.email=drive@semlith.invalid",
                    "-c",
                    "user.name=semlith drive",
                    "commit",
                    "--quiet",
                    "-m",
                    "the commit the drive fixture needs",
                ],
                cwd=directory,
            )
        except FixtureError as error:
            raise FixtureError(
                "the projects fixture needs git on PATH, because the picker it "
                "exercises finds projects by looking for repositories: %s" % error
            )
        return directory

    # -------------------------------------------------------------- corpora

    def adoptme(self):
        """A folder holding a valid `.semlith` store, for finding 2.1 and 4.18.

        The store is built by the binary under test rather than assembled
        here, because "is this a semlith store" is the product's question to
        answer and a hand-built directory would only prove this file can copy
        a schema.
        """
        # Rebuilt rather than cached once the store has gone: adopting *moves*
        # the `.semlith` into the home, so the folder finding 2.1 hands back is
        # not a folder finding 4.18 can still find a store in.
        if self._adoptme and os.path.exists(
            os.path.join(self._adoptme, ".semlith", "store.db")
        ):
            return self._adoptme
        self._adopted += 1
        directory = os.path.join(self.root, "adoptme-%d" % self._adopted)
        os.makedirs(directory, exist_ok=True)
        with open(os.path.join(directory, "README.md"), "w", encoding="utf-8") as handle:
            handle.write(DOC % {"name": "adoptme"})
        with open(os.path.join(directory, "orphan.py"), "w", encoding="utf-8") as handle:
            handle.write("def orphan():\n    return 'a file with a symbol in it'\n")
        store = os.path.join(directory, ".semlith")
        run([semlith_bin(), "--store", store, "index", directory])
        if not os.path.exists(os.path.join(store, "store.db")):
            raise FixtureError(
                "building the adopt fixture produced no store.db in %s" % store
            )
        self._adoptme = directory
        return directory

    def monorepo(self):
        """Two git repositories and one plain folder, for finding 2.5.

        Deliberately not at `$HOME`'s top level: the whole point of 2.5 is that
        the picker could not be pointed anywhere else, so the check has to walk
        it somewhere. The portal's `/api/dirs` and `/api/projects` still
        confine the pickers to the home directory — correctly — which is what
        `corpus_root` above keeps the whole corpus inside.
        """
        if self._monorepo:
            return self._monorepo
        directory = os.path.join(self.root, "mono")
        os.makedirs(directory, exist_ok=True)
        self._git_repo(os.path.join(directory, "repo-one"))
        self._git_repo(os.path.join(directory, "repo-two"))
        self._write_corpus(os.path.join(directory, "plain-folder"), 3, prefix="plain")
        self._monorepo = directory
        return directory

    def bulk(self, count=600):
        """A corpus large enough for the live chunk counter to be watched.

        Six hundred, because that is the run in finding 1.5 where the counter
        wrapped, and the wrap was at a flush boundary rather than at a file
        count — a smaller corpus never reaches one.
        """
        if self._bulk:
            return self._bulk
        directory = os.path.join(self.root, "bulk")
        self._write_corpus(directory, count, prefix="bulk")
        self._bulk = directory
        return directory

    def small(self):
        """Three files, matching the `proj-one` shape in findings 1.3 and 1.6."""
        if self._small:
            return self._small
        directory = os.path.join(self.root, "small")
        self._write_corpus(directory, 3, prefix="small")
        self._small = directory
        return directory

    def second(self):
        """A second three-file corpus, so two stores can be open at once.

        Finding 1.2 is about what a cross-store query does to the ledger, and a
        drive that starts against an empty store home has no second store to
        cross. A separate directory rather than `small()` twice: indexing one
        directory twice makes one store, which is the thing 1.2 cannot use.
        """
        if self._second:
            return self._second
        self._second = self._write_corpus(
            os.path.join(self.root, "second"), 3, prefix="second"
        )
        return self._second

    def unique(self, prefix="solo"):
        """A corpus nobody else indexes, so the store it makes is this call's.

        For a check that has to name one card on a live page (finding 2.6): the
        Index page grows cards from every other check's runs while this one
        watches, and a store only this call indexes is the one handle on the
        page that stays a handle.
        """
        self._unique += 1
        return self._write_corpus(
            os.path.join(self.root, "%s-%d" % (prefix, self._unique)), 3, prefix=prefix
        )

    def doomed(self):
        """A corpus meant to be deleted out from under its store, for 3.1.

        A registry entry whose root has gone is the "dead store" the findings
        describe, and this is the safe half of producing one: the root is
        removed, never the store directory the daemon holds open. Removing a
        directory the daemon has mapped is a Windows sharing violation waiting
        to happen, and the badge under test is driven by `roots[].present`
        either way.
        """
        if self._doomed:
            return self._doomed
        directory = os.path.join(self.root, "doomed")
        self._write_corpus(directory, 2, prefix="doomed")
        self._doomed = directory
        return directory

    def remove_doomed(self):
        shutil.rmtree(self.doomed(), ignore_errors=True)


if __name__ == "__main__":
    # A self-check for the fixture builder alone: everything but `adoptme`,
    # which needs the binary. Run it when the drive fails during setup and you
    # want to know whether the corpora or the daemon is at fault.
    made = Fixtures()
    try:
        small = made.small()
        names = os.listdir(small)
        if len(names) != 3:
            raise SystemExit("expected 3 files in the small corpus, got %d" % len(names))
        bulk = made.bulk(20)
        if len(os.listdir(bulk)) != 20:
            raise SystemExit("the bulk corpus did not come out the size asked for")
        mono = made.monorepo()
        for child in ("repo-one", "repo-two", "plain-folder"):
            if not os.path.isdir(os.path.join(mono, child)):
                raise SystemExit("the monorepo fixture is missing %s" % child)
        print("fixtures.py self-check passed (%s)" % made.root)
    finally:
        made.cleanup()
