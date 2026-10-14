"""HTTP client for plugins.

The standard library only, on purpose. A plugin runs in-process holding the
GIL, so third-party HTTP stacks with connection pools or background threads are
a liability: a hung socket freezes every other plugin.

``fetch`` is therefore a thin, timeout-bounded wrapper over ``urllib``. It does
not hold the GIL-releasing machinery that a Rust HTTP client would, but it does
enforce a hard deadline so a slow provider cannot stall the server.
"""

from __future__ import annotations

import json
import urllib.error
import urllib.parse
import urllib.request
from typing import Any

__all__ = ["fetch", "fetch_json", "HttpError"]

DEFAULT_TIMEOUT = 10.0
USER_AGENT = "jellyfin-clone-plugin/1.0"


class HttpError(RuntimeError):
    """An HTTP request failed, or returned a non-2xx status."""

    def __init__(self, status: int, url: str, body: str = "") -> None:
        self.status = status
        self.url = url
        self.body = body
        super().__init__(f"HTTP {status} from {url}: {body[:200]}")


def fetch(
    url: str,
    *,
    params: dict[str, Any] | None = None,
    headers: dict[str, str] | None = None,
    timeout: float = DEFAULT_TIMEOUT,
    method: str = "GET",
    data: bytes | None = None,
) -> str:
    """GET/POST a URL and return the body as text.

    Raises :class:`HttpError` on a non-2xx response, and ``TimeoutError`` when
    the deadline passes.
    """
    if params:
        sep = "&" if "?" in url else "?"
        url = f"{url}{sep}{urllib.parse.urlencode(params, doseq=True)}"

    request = urllib.request.Request(url, data=data, method=method)
    request.add_header("User-Agent", USER_AGENT)
    request.add_header("Accept", "application/json")
    for key, value in (headers or {}).items():
        request.add_header(key, value)

    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace") if exc.fp else ""
        raise HttpError(exc.code, url, body) from None
    except urllib.error.URLError as exc:
        if isinstance(exc.reason, TimeoutError):
            raise TimeoutError(f"request to {url} timed out after {timeout}s") from None
        raise HttpError(0, url, str(exc.reason)) from None


def fetch_json(url: str, **kwargs: Any) -> Any:
    """:func:`fetch` with the body parsed as JSON."""
    return json.loads(fetch(url, **kwargs))