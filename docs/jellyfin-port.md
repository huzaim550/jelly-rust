# Porting from upstream Jellyfin

This project reimplements Jellyfin, so wherever possible we copy Jellyfin's
behaviour instead of inventing our own. This document records what has been
ported, where it lives, and where the remaining work should come from.

## Reference source

Two shallow clones live under `reference/` (git-ignored, not built):

| Path                       | Upstream                                   | Use |
| -------------------------- | ------------------------------------------ | --- |
| `reference/jellyfin-web/`  | https://github.com/jellyfin/jellyfin-web   | TypeScript/React client: themes, layout, components |
| `reference/jellyfin/`      | https://github.com/jellyfin/jellyfin        | C# server: API, naming, metadata, providers |

Re-create them with:

```sh
git clone --depth 1 --single-branch https://github.com/jellyfin/jellyfin-web.git reference/jellyfin-web
git clone --depth 1 --single-branch https://github.com/jellyfin/jellyfin.git     reference/jellyfin
```

The running image (`jellyfin/jellyfin:latest`) also ships compiled assets. The
only thing worth reading there is the design tokens: each scheme has a compiled
`/jellyfin/jellyfin-web/themes/<scheme>/theme.css` whose `var(--jf-palette-…)`
fallbacks are the exact shipped values.

---

## 1. Design tokens & themes — **ported**

Jellyfin's theme system is a `data-theme` attribute on `<html>` plus `--jf-*`
CSS variables (MUI config: `colorSchemeSelector: '[data-theme="%s"]'`,
`cssVarPrefix: 'jf'`). We mirror that exactly.

| Where (ours)            | Where (upstream) |
| ----------------------- | ---------------- |
| `web/src/theme.css`     | `src/themes/_base/_palette.scss`, `src/themes/_base/_theme.scss`, `src/themes/<scheme>/theme.scss` |
| `web/src/theme.ts`      | `src/themes/index.ts`, `src/themes/themeStorageManager.ts`, `src/scripts/themeManager.js` |
| `web/index.html`        | pre-paint theme bootstrap |

Ported schemes (Jellyfin's six, plus this client's original palette kept as
`cinema`): `dark` (default), `light`, `appletv`, `blueradiance`, `purplehaze`,
`wmc`, `cinema`. Values were taken from the source SCSS and cross-checked
against the compiled CSS in the image.

`theme.css` keeps Jellyfin's variable names (`--jf-palette-primary-main`,
`--jf-card-borderRadius`, …) and then **bridges** them onto the semantic tokens
the component stylesheet already uses (`--bg`, `--accent`, `--text`, …). That
bridge is why flipping one attribute re-skins the whole client without editing
`styles.css`.

Noto Sans is self-hosted from `web/public/fonts/` (copied out of the image), so
the type matches without a Google Fonts request.

### Adding a theme

1. Add a `[data-theme="<id>"]` block to `theme.css` with the `--jf-*` values
   (copy them from the matching `reference/jellyfin-web/src/themes/<id>/theme.scss`).
2. Add an entry to `THEMES` in `theme.ts` with a label and swatch.

The picker in the account menu and persistence come for free.

### Still to port (web UI structure)

The token layer re-skins the existing components, but Jellyfin's *structural*
patterns are not yet adopted. Candidates, with upstream references:

- Library/card grid and card overlay buttons — `src/components/cardbuilder/`,
  `src/components/card/`.
- Item detail page: backdrop ribbon, metadata sidebar, action row —
  `src/elements/emby-*` and `src/apps/.../itemDetail`.
- Media player chrome (OSD, subtitle menu, next-up) — `src/plugins/htmlVideoPlayer/`,
  `src/components/nowPlayingBar/`.
- Nav drawer grouping semantics — `src/components/mainDrawer/`.

---

## 2. Server logic / API — **already mirrored, hardening remains**

Our `crates/jf-http` already follows Jellyfin's route shapes and PascalCase DTOs
(the C# reference is cited throughout the code comments). Useful upstream
starting points when a route needs to match more closely:

| Ours                     | Upstream |
| ------------------------ | -------- |
| `crates/jf-http/src/auth.rs`   | `Jellyfin.Api/Auth/`, `MediaBrowser.Controller/Session` |
| `crates/jf-http/src/users.rs`  | `Jellyfin.Api/Controllers/UserController.cs`, `UserPolicy` |
| `crates/jf-http/src/routes.rs` | `Jellyfin.Api/Controllers/*` |
| `crates/jf-db/src/users.rs`    | `Jellyfin.Data/` (user policy fields are the contract) |

Highest-value ports: session/device registration semantics, the exact
`UserPolicy` field set and defaults, and transcoding/playback negotiation
(`MediaBrowser.MediaEncoding/`).

---

## 3. Metadata & scanning — **partially reimplemented, harden from upstream**

`crates/jf-scan` already parses movies/episodes and reads Kodi NFOs. Jellyfin's
parser is regex-driven and covers many more forms; port it incrementally.

| Ours (Rust)                  | Upstream (C#) |
| ---------------------------- | ------------- |
| `crates/jf-scan/src/naming.rs` | `Emby.Naming/Video/VideoResolver.cs`, `Emby.Naming/TV/EpisodePathParser.cs`, `Emby.Naming/Common/NamingOptions.cs` (the canonical `CleanStrings`, media extensions, episode expressions) |
| `crates/jf-scan/src/nfo.rs`    | `MediaBrowser.LocalMetadata/`, `MediaBrowser.XbmcMetadata/` |
| `crates/jf-scan/src/walk.rs`   | `Emby.Server.Implementations/Library/`, `MediaBrowser.Controller/Resolvers` |

Concrete gaps worth copying next: Kodi clean strings ("Director's Cut",
"Extended", "Uncut"), date-based and absolute-numbered episodes, stacked files
(`FileStackRule`), extras/trailers (`ExtraRuleResolver`), and the authoritative
extension lists in `NamingOptions`.

These need a regex engine in `jf-scan`; `NamingOptions.cs` is the source of the
patterns.

---

## Verification

Web changes are covered by `web/test/` (`theme.test.tsx` covers the theme
manager and picker). Rust changes are covered by the `#[cfg(test)]` modules
next to each file plus `crates/jf-http/tests/`.

```sh
cd web && npm run typecheck && npx vitest run && npm run build
cargo test --workspace
```
