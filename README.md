# herdr-extras

Personal [herdr](https://herdr.dev) TUI plugins, one subdirectory per plugin, each installable on its own.

| Plugin | What it does |
| --- | --- |
| [`trail`](plugins/trail/README.md) | Agent activity for herdr: a lane per agent pane with state, context and cost, and a step-through session timeline |

## Install

Each plugin's README has its install command and options. For trail:

```
herdr plugin install haitheredavid/herdr-extras/plugins/trail --ref trail-v0.1.0
```

## Layout

Each `plugins/<name>/` is a self-contained crate with its own `herdr-plugin.toml`, `Cargo.lock`, `README.md` and `CHANGELOG.md`. There is no Cargo workspace: herdr builds and runs each plugin from its own subdirectory. Releases are tagged per plugin (`trail-v0.1.0`).

## trail

See [plugins/trail/README.md](plugins/trail/README.md) for views, keys, configuration, data handling and development.
