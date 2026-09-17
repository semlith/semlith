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

import os
import shutil
import subprocess
import tempfile


class FixtureError(Exception):
    """A corpus could not be built, which is the harness's fault, not the product's."""


def semlith_bin():
    return os.environ.get("SEMLITH_BIN", "semlith")


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
        self.root = tempfile.mkdtemp(prefix="semlith-drive-corpus-")
        self.keep = keep
        self._adoptme = None
        self._monorepo = None
        self._bulk = None
        self._small = None
        self._doomed = None

    def cleanup(self):
        if self.keep:
            return
        shutil.rmtree(self.root, ignore_errors=True)

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
        if self._adoptme:
            return self._adoptme
        directory = os.path.join(self.root, "adoptme")
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

        Deliberately not under `$HOME`'s top level: the whole point of 2.5 is
        that the picker could not be pointed anywhere else. If the portal's
        `/api/dirs` still confines the picker to the home directory — which
        it does, and correctly so — the drive's temp root has to be inside it,
        which is what `SEMLITH_DRIVE_ROOT` is for. See the README.
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
