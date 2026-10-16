from __future__ import annotations

import collections.abc as cabc
import inspect
import os
import sys
import typing as t
import weakref
from datetime import timedelta
from functools import update_wrapper
from inspect import iscoroutinefunction
from itertools import chain
from types import TracebackType
from urllib.parse import quote as _url_quote
from urllib.parse import urlsplit

import click
from werkzeug.datastructures import Headers
from werkzeug.exceptions import BadRequestKeyError
from werkzeug.exceptions import HTTPException
from werkzeug.exceptions import InternalServerError
from werkzeug.routing import BuildError
from werkzeug.routing import MapAdapter
from werkzeug.routing import RequestRedirect
from werkzeug.routing import RoutingException
from werkzeug.routing import Rule
from werkzeug.serving import is_running_from_reloader
from werkzeug.wrappers import Response as BaseResponse
from werkzeug.wsgi import get_host

from . import cli
from . import typing as ft
from .ctx import AppContext


class Flask(App):
    """The flask object implements a WSGI application."""

    def make_response(self, rv: ft.ResponseReturnValue) -> Response:
        """Convert the return value from a view function to a response."""
        return self.response_class(rv)

    def full_dispatch_request(self) -> Response:
        rv = self.dispatch_request()
        return self.make_response(rv)
