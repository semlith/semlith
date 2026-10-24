"""Self-test for the drive runner's one retry rule.

The Windows runner's browser sometimes cannot send a loopback request at all
(net::ERR_NO_BUFFER_SPACE, WSAENOBUFS). The daemon never sees it, so a check
that fails while that is in the page's errors failed for the runner, not the
portal, and the runner runs it once more. Every other failure stays a failure,
which is what this file holds the rule to.

    python3 tests/drive/drive_test.py
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import drive  # noqa: E402

UNSENT = (
    "07:18:03 Failed to load resource: net::ERR_NO_BUFFER_SPACE "
    "http://127.0.0.1:7466/api/index/runs"
)

# The lines that are a reason to retry: the exact fault, wherever it sits.
assert drive.unsent_request([UNSENT]) == UNSENT
assert drive.unsent_request(["07:18:01 TypeError: x is undefined", UNSENT]) == UNSENT

# Everything else is the portal's failure and must not be retried.
assert drive.unsent_request([]) is None
assert drive.unsent_request(["07:18:03 TypeError: Cannot read properties of null"]) is None
assert drive.unsent_request(
    ["07:18:03 Failed to load resource: net::ERR_CONNECTION_REFUSED http://127.0.0.1:7466/api/stores"]
) is None
assert drive.unsent_request(
    ["07:18:03 Failed to load resource: the server responded with a status of 500 () http://127.0.0.1:7466/api/stores"]
) is None

print("drive_test: ok")
