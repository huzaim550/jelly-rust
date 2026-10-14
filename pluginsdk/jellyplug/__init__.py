"""jellyplug — the Python SDK for writing jellyfin-clone plugins.

A plugin is a Python module with a ``manifest.toml`` beside it. The host
injects three globals before your ``setup()`` runs:

``host``
    Logging, the host API version, and read access to items.
``config``
    Settings namespaced to your plugin, persisted for you.
``capabilities``
    The capability strings your manifest requested.

The minimum viable plugin::

    import jellyplug

    def setup():
        host.log("info", "tmdb provider ready")
        config.set("api_key", "...")

    def fetch_metadata(query_json: str) -> str | None:
        query = jellyplug.parse_query(query_json)
        data = tmdb_search(query.title, query.year)
        return jellyplug.dumps(jellyplug.MetadataResult(
            title=data["title"],
            overview=data["overview"],
        ))

    def on_event(event_json: str) -> None:
        event = jellyplug.parse_event(event_json)
        ...

Contract notes that matter:

* ``fetch_metadata`` receives and returns **JSON strings**, not Python
  objects. The bridge is deliberately narrow; keeping it string-only means
  the host never needs to serialize arbitrary Python objects.
* Returning ``None`` or ``""`` means "I have nothing", and the host asks the
  next provider in the chain.
* Hooks run on a dedicated thread pool and hold the GIL. **Never block** in a
  hook: it stalls every other plugin on the server. Use ``jellyplug.fetch``
  for I/O instead of a bare ``requests`` call.
* Plugins must be pure Python. A crash in a native extension cannot be
  caught by the host and takes the whole server with it.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field, asdict
from typing import Any

__all__ = [
    "MetadataResult",
    "MetadataPerson",
    "PersonKind",
    "MetadataImage",
    "ImageKind",
    "MetadataQuery",
    "PluginEvent",
    "parse_query",
    "parse_event",
    "dumps",
    "load_manifest",
    "require_capability",
]

__version__ = "1.0.0"

# The host API version this SDK targets. The host refuses plugins declaring a
# different major version, so a mismatch fails loudly at load time rather than
# mysteriously at runtime.
API_VERSION = "1.0"


# --------------------------------------------------------------------------
# Data shapes — must stay in sync with crates/jf-plugins/src/api.rs
# --------------------------------------------------------------------------


class PersonKind:
    ACTOR = "actor"
    DIRECTOR = "director"
    WRITER = "writer"
    PRODUCER = "producer"
    GUEST_STAR = "guest_star"


class ImageKind:
    PRIMARY = "primary"
    BACKDROP = "backdrop"
    LOGO = "logo"
    BANNER = "banner"
    THUMB = "thumb"


@dataclass
class MetadataPerson:
    name: str
    role: str = ""
    kind: str = PersonKind.ACTOR


@dataclass
class MetadataImage:
    """An artwork reference.

    ``path`` may be an absolute URL the host will download, or a filename
    relative to the media file (e.g. ``poster.jpg``) for sidecar art.
    """

    path: str
    kind: str = ImageKind.PRIMARY
    index: int = 0
    width: int | None = None
    height: int | None = None


@dataclass
class MetadataResult:
    """What a provider returns. Every field is optional; fill in what you know."""

    title: str | None = None
    original_title: str | None = None
    sort_title: str | None = None
    overview: str | None = None
    tagline: str | None = None
    year: int | None = None
    premiere_date: str | None = None
    community_rating: float | None = None
    critic_rating: float | None = None
    official_rating: str | None = None
    status: str | None = None
    genres: list[str] = field(default_factory=list)
    studios: list[str] = field(default_factory=list)
    run_time_ticks: int | None = None
    people: list[MetadataPerson] = field(default_factory=list)
    images: list[MetadataImage] = field(default_factory=list)
    external_ids: dict[str, str] = field(default_factory=dict)

    def is_empty(self) -> bool:
        """True when nothing was resolved, so the host can skip the write."""
        return not any(
            [
                self.title,
                self.original_title,
                self.sort_title,
                self.overview,
                self.tagline,
                self.year,
                self.premiere_date,
                self.community_rating,
                self.critic_rating,
                self.official_rating,
                self.status,
                self.genres,
                self.studios,
                self.run_time_ticks,
                self.people,
                self.images,
                self.external_ids,
            ]
        )


@dataclass
class MetadataQuery:
    """What the host asks a provider to resolve."""

    title: str
    year: int | None = None
    kind: str = "Movie"
    known_ids: dict[str, str] = field(default_factory=dict)
    preferred_languages: list[str] = field(default_factory=list)

    @property
    def is_episode(self) -> bool:
        return self.kind == "Episode"

    def known_id(self, provider: str) -> str | None:
        return self.known_ids.get(provider)


@dataclass
class PluginEvent:
    """An event delivered to ``on_event``."""

    type: str
    payload: dict[str, Any] = field(default_factory=dict)

    def __repr__(self) -> str:
        return f"PluginEvent({self.type!r}, {self.payload!r})"


# --------------------------------------------------------------------------
# Host interop helpers
# --------------------------------------------------------------------------


def _host_globals() -> dict[str, Any]:
    """Return the globals of the module that called into the SDK.

    The host injects ``host``, ``config``, ``capabilities`` and ``plugin_id``
    into each *plugin* module's namespace, not into this one. So ``globals()``
    evaluated inside the SDK is the wrong namespace entirely, and using it would
    silently see nothing. Walking one frame up to the caller's module is the
    only way to find what the host actually injected.
    """
    import sys

    try:
        # _getframe(0) is this helper; _getframe(1) is the SDK function that
        # called it; _getframe(2) is the plugin.
        return sys._getframe(2).f_globals
    except (AttributeError, ValueError):
        # Not CPython, or called from the top level.
        return {}


def dumps(obj: Any) -> str:
    """Serialize a dataclass or dict for return from a hook."""
    if hasattr(obj, "__dataclass_fields__"):
        obj = asdict(obj)
    return json.dumps(obj, separators=(",", ":"))


def parse_query(raw: str) -> MetadataQuery:
    """Parse the JSON query passed to ``fetch_metadata``."""
    data = json.loads(raw)
    return MetadataQuery(
        title=data.get("title", ""),
        year=data.get("year"),
        kind=data.get("kind", "Movie"),
        known_ids=data.get("known_ids") or {},
        preferred_languages=data.get("preferred_languages") or [],
    )


def parse_event(raw: str) -> PluginEvent:
    """Parse the JSON payload passed to ``on_event``."""
    data = json.loads(raw)
    return PluginEvent(type=data.get("type", "unknown"), payload=data.get("payload") or {})


def load_manifest(path: str = "manifest.toml") -> dict:
    """Read the plugin's own manifest, for self-describing UIs."""
    try:
        import tomllib
    except ImportError:  # Python < 3.11
        raise RuntimeError("jellyplug requires Python 3.11+ for tomllib") from None

    with open(path, "rb") as fh:
        return tomllib.load(fh)


def require_capability(name: str) -> None:
    """Raise if the plugin did not declare a capability it is about to use.

    Calling this at the top of a hook turns a confusing permission error into
    an actionable message, and documents the requirement inline.
    """
    scope = _host_globals()
    granted = scope.get("capabilities") or []
    if name not in granted:
        raise PermissionError(
            f"plugin {scope.get('plugin_id', '?')!r} needs the {name!r} "
            f"capability. Add it to the `capabilities` list in manifest.toml. "
            f"Granted: {granted}"
        )


def log(level: str, message: str) -> None:
    """Log through the host, falling back to stderr if run standalone."""
    host = _host_globals().get("host")
    if host is not None:
        host.log(level, message)
    else:
        import sys

        print(f"[{level}] {message}", file=sys.stderr)