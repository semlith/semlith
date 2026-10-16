from __future__ import annotations

import importlib.util
import os
import sys
import typing as t
from datetime import datetime
from functools import cache
from functools import update_wrapper
from types import TracebackType

import werkzeug.utils
from werkzeug.exceptions import abort as _wz_abort
from werkzeug.utils import redirect as _wz_redirect
from werkzeug.wrappers import Response as BaseResponse

from .globals import _cv_app
from .globals import current_app


def make_response(*args: t.Any) -> Response:
    """Sometimes it is necessary to set additional headers in a view."""
    if not args:
        return current_app.response_class()
    if len(args) == 1:
        args = args[0]
    return current_app.make_response(args)
