"""A small virtual DOM.

There is no usable browser UI library for Python, and pulling a JavaScript
framework into a Python frontend would defeat the point. So: ~200 lines of
diffing, which is genuinely all a media-server UI needs.

Design:

* :class:`VNode` is an immutable description of a DOM subtree.
* :meth:`App.render` diffs the new tree against the mounted one and applies the
  smallest set of ``createElement`` / ``setAttribute`` / ``textContent``
  operations that reconciles them.
* Nodes carry a stable ``key`` so that reordering a list (the common case in a
  media grid, where items stream in as metadata resolves) moves existing DOM
  nodes rather than recreating them. Without this, every metadata update would
  reset scroll position and re-download images.

Events are bound through the ``on`` mapping and stored on the DOM node itself,
so a re-render never leaks listeners onto discarded nodes.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Callable

# Element tag -> attribute name for boolean attributes, which HTML treats as
# presence/absence rather than true/false.
BOOLEAN_ATTRS = {
    "disabled", "checked", "selected", "readonly", "autofocus", "hidden",
    "required", "multiple", "open", "controls", "loop", "muted", "playsinline",
}

# Attribute name -> DOM property, where the property is the correct target.
PROPERTY_OVERRIDES = {
    "value": "value",
    "checked": "checked",
    "selected": "selected",
    "src": "src",
}


@dataclass
class VNode:
    """One node in the virtual tree.

    ``tag`` is a lowercase element name, or a callable returning a
    :class:`VNode` for components.
    """

    tag: str | Callable[..., "VNode"]
    attrs: dict[str, Any] = field(default_factory=dict)
    children: list["VNode | str"] = field(default_factory=list)
    #: Stable identity across renders, used to move rather than rebuild nodes.
    key: str | None = None
    #: Escape hatch for direct DOM access after mount.
    ref: Callable[[Any], None] | None = None

    @property
    def is_component(self) -> bool:
        return callable(self.tag)


def h(tag: str, *children, key: str | None = None, ref=None, **attrs) -> VNode:
    """Shorthand constructor.

    ``h("div", h("span", "hi"), class_="row")``
    """
    # `class` is a keyword, so callers pass class_; normalize here.
    if "class_" in attrs:
        attrs["class"] = attrs.pop("class_")

    # Flatten nested lists of children and drop None/False, which are
    # conditional-render artifacts.
    flat: list[VNode | str] = []
    for child in children:
        if child is None or child is False:
            continue
        if isinstance(child, (list, tuple)):
            flat.extend(c for c in child if c is not None and c is not False)
        else:
            flat.append(child)

    return VNode(tag=tag, attrs=attrs, children=flat, key=key, ref=ref)


class App:
    """Mounts and re-renders a virtual tree into a real DOM element."""

    def __init__(self, root, doc=None):
        self.root = root
        self._doc = doc
        self._mounted: VNode | None = None
        self._dom_cache: dict[int, Any] = {}

    def _document(self):
        if self._doc is None:
            from .shim import document

            self._doc = document
        return self._doc

    def render(self, tree: VNode) -> None:
        """Diff ``tree`` against the mounted tree and patch the DOM."""
        if self._mounted is None:
            # First paint: build the whole subtree.
            element = self._create(tree)
            if element is not None:
                root = self.root
                # Clear anything the loader left behind.
                while root.first_child:
                    root.removeChild(root.first_child)
                root.appendChild(element)
        else:
            self._patch(self._mounted, tree, self.root, None)

        self._mounted = tree

    # ------------------------------------------------------------------
    # Creating
    # ------------------------------------------------------------------

    def _create(self, node: VNode | str):
        if isinstance(node, str):
            return self._document().createTextNode(node)

        if node.is_component:
            # Components are plain functions; render them into a VNode first.
            return self._create(node.tag(*_component_args(node)))

        doc = self._document()
        element = doc.createElement(node.tag)

        if node.key is not None:
            element.dataset.key = node.key
        self._apply_attrs(element, {}, node.attrs)
        self._dom_cache[id(node)] = element

        for child in node.children:
            created = self._create(child)
            if created is not None:
                element.appendChild(created)

        if node.ref:
            node.ref(element)
        return element

    def _apply_attrs(self, element, old: dict, new: dict) -> None:
        for name in old:
            if name not in new and not name.startswith("on"):
                _remove_attr(element, name)

        for name, value in new.items():
            if name.startswith("on"):
                # Listeners live on the DOM node, so re-render never stacks
                # duplicates on a reused element.
                element[name] = value
                continue
            if old.get(name) == value:
                continue
            _set_attr(element, name, value)

        # Clear a removed event handler so clicks do not fire stale handlers.
        for name in old:
            if name.startswith("on") and name not in new:
                element[name] = None

    # ------------------------------------------------------------------
    # Diffing
    # ------------------------------------------------------------------

    def _patch(self, old: VNode, new: VNode, parent, anchor) -> None:
        """Reconcile ``old`` into ``new`` beneath ``parent``, before ``anchor``."""
        if old.tag != new.tag:
            # Different element type: replace wholesale.
            created = self._create(new)
            if created is not None:
                parent.replaceChild(created, self._dom_cache.get(id(old)) or _first_child(parent))
            return

        if isinstance(new.tag, str):
            element = self._dom_cache.get(id(old))
            if element is None:
                # Lost track of the element (cache miss); safest is a rebuild.
                self._mount_fallback(parent, new, anchor)
                return

            self._apply_attrs(element, old.attrs, new.attrs)

            if new.ref:
                new.ref(element)

            self._patch_children(element, old.children, new.children)
            self._dom_cache[id(new)] = element
            # The old VNode is discarded; drop its mapping so the cache does
            # not grow without bound across renders.
            self._dom_cache.pop(id(old), None)

    def _patch_children(self, element, old_children, new_children) -> None:
        """Reconcile child lists, using keys to minimize DOM churn."""
        old_by_key: dict[str, VNode] = {}
        for child in old_children:
            if isinstance(child, VNode) and child.key is not None:
                old_by_key[child.key] = child

        # Track which real nodes are still in play, so extras can be removed.
        reused = set()

        for index, new_child in enumerate(new_children):
            if isinstance(new_child, VNode) and new_child.key is not None:
                match = old_by_key.get(new_child.key)
            else:
                match = old_children[index] if index < len(old_children) else None

            if isinstance(match, VNode) and isinstance(new_child, VNode):
                if match.tag == new_child.tag:
                    reused.add(id(match))
                    self._patch(match, new_child, element, None)
                    self._position(element, new_child, index)
                    continue
                # Same key, different tag: fall through to a fresh create.

            created = self._create(new_child)
            if created is None:
                continue
            element.appendChild(created)
            self._position(element, new_child, index)

        # Drop children that are no longer present.
        for child in old_children:
            if not isinstance(child, VNode) or id(child) in reused:
                continue
            element_node = self._dom_cache.get(id(child))
            if element_node is not None:
                try:
                    element.removeChild(element_node)
                except Exception:  # noqa: BLE001 - already detached
                    pass
                self._dom_cache.pop(id(child), None)

    def _position(self, parent, node: VNode, index: int) -> None:
        """Move a node to ``index`` if it is not already there."""
        element = self._dom_cache.get(id(node))
        if element is None:
            return
        current = parent.childNodes
        if index >= current.length:
            return
        if current.item(index) is element:
            return
        try:
            parent.insertBefore(element, current.item(index))
        except Exception:  # noqa: BLE001 - cross-document moves can throw
            pass

    def _mount_fallback(self, parent, new: VNode, anchor) -> None:
        created = self._create(new)
        if created is None:
            return
        if anchor is not None:
            parent.insertBefore(created, anchor)
        else:
            parent.appendChild(created)


def _component_args(node: VNode) -> tuple:
    """Turn a component's attrs into keyword arguments."""
    attrs = dict(node.attrs)
    attrs["children"] = node.children
    if node.key is not None:
        attrs["key"] = node.key
    if node.ref is not None:
        attrs["ref"] = node.ref
    return (attrs,)


def _set_attr(element, name: str, value: Any) -> None:
    """Set an attribute or the correct DOM property."""
    if value is None or value is False:
        _remove_attr(element, name)
        return

    if name in PROPERTY_OVERRIDES:
        # `value` and `checked` must be set as properties to take effect.
        setattr(element, PROPERTY_OVERRIDES[name], value)
        return

    if name == "style" and isinstance(value, dict):
        for key, val in value.items():
            element.style[key] = val
        return

    if name in BOOLEAN_ATTRS:
        element.setAttribute(name, "")
        return

    if name.startswith("data_"):
        element.dataset[name[5:].replace("_", "-")] = str(value)
        return

    element.setAttribute(name, str(value))


def _remove_attr(element, name: str) -> None:
    if name in PROPERTY_OVERRIDES:
        setattr(element, PROPERTY_OVERRIDES[name], "" if name != "checked" else False)
        return
    if name.startswith("data_"):
        try:
            del element.dataset[name[5:].replace("_", "-")]
        except Exception:  # noqa: BLE001
            pass
        return
    element.removeAttribute(name)


def _first_child(parent):
    return parent.firstChild if parent.firstChild else None