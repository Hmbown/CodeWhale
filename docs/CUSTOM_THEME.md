# Custom Themes

DeepSeek TUI supports custom theme files that inherit from a built-in preset and
override individual visual tokens.  Theme files are plain TOML and live in the
user config directory; the theme picker (`/theme`) discovers them automatically.

## Quick Start

Copy `theme.example.toml` (included in the repository root) into the themes
directory and tweak it:

```bash
mkdir -p ~/.config/deepseek/themes
cp theme.example.toml ~/.config/deepseek/themes/mo.toml
```

Activate it:

```
/theme mo
```

Or persist it in `~/.deepseek/config.toml`:

```toml
theme = "file:mo"
```

## Where Themes Live

Custom theme files are stored as TOML files under:

```
~/.config/deepseek/themes/<name>.toml
```

The file stem (the part before `.toml`) becomes the theme name.  For example,
`midnight.toml` is referenced as `file:midnight` in config and shown as
**Midnight (custom)** in the theme picker.

The directory is created automatically if it doesn't exist when a theme file is
saved, but you can also create it by hand:

```bash
mkdir -p ~/.config/deepseek/themes
```

## File Format

Every custom theme file **must** declare a `base` key that names one of the
built-in presets.  Every other key is optional — omitted keys inherit from the
base theme.

```toml
# Required: built-in theme to inherit from
base = "dark"

# Optional: any override from the full palette
background_color = "#0D1117"
border_type     = "rounded"
```

### Built-in Base Presets

| `base` value               | Theme            | Mode      | Description                     |
|----------------------------|------------------|-----------|---------------------------------|
| `system`                   | System           | Auto      | Follows terminal background     |
| `dark`                     | Whale (Dark)     | Dark      | Original DeepSeek dark default  |
| `light`                    | Whale Light      | Light     | Light variant of the Whale theme |
| `grayscale`                | Grayscale        | Grayscale | Low-opinion black/white         |
| `catppuccin-mocha`         | Catppuccin Mocha | Dark      | Community preset                |
| `tokyo-night`              | Tokyo Night      | Dark      | Community preset                |
| `dracula`                  | Dracula          | Dark      | Community preset                |
| `gruvbox-dark`             | Gruvbox Dark     | Dark      | Community preset                |

Aliases such as `whale`, `mono`, `black-white`, `tokyonight`, and `gruvbox` are
accepted as `base` values.

## All Configurable Fields

Every field is optional except `base`.  Unset fields inherit from the base
theme.  Color values accept `#RRGGBB`, `RRGGBB` (without `#`), or the keyword
`"reset"` (clears the override and falls back to the base theme's value).

### Backgrounds

| Field              | Description                                          |
|--------------------|------------------------------------------------------|
| `background_color` | Main TUI background (also sets header/footer)        |
| `panel_bg`         | Side panel background                                |
| `sidebar_bg`       | Sidebar panel background (defaults to `panel_bg`)    |
| `elevated_bg`      | Elevated surface background                          |
| `composer_bg`      | Input-area background                                |
| `selection_bg`     | Selection highlight background                       |
| `header_bg`        | Header bar background (inherits `background_color`)  |
| `footer_bg`        | Footer bar background (inherits `background_color`)  |

### Borders

| Field              | Description                                 |
|--------------------|---------------------------------------------|
| `border_type`      | `"plain"` (default) or `"rounded"`          |
| `border_color`     | Border line colour (`#RRGGBB`)              |

### Mode Status Colours

| Field         | Description           |
|---------------|-----------------------|
| `mode_agent`  | Agent mode indicator  |
| `mode_yolo`   | YOLO mode indicator   |
| `mode_plan`   | Plan mode indicator   |

### Status Indicator Colours

| Field            | Description        |
|------------------|--------------------|
| `status_ready`   | Idle / ready       |
| `status_working` | Active / processing |
| `status_warning` | Warning attention   |

### Text Colours

| Field       | Description      |
|-------------|------------------|
| `text_dim`  | Dimmed text      |
| `text_hint` | Hint text        |
| `text_muted`| Muted text       |
| `text_body` | Body / primary   |
| `text_soft` | Soft text        |

### Section / Tool Colours

| Field                   | Description              |
|-------------------------|--------------------------|
| `section_border_color`  | Section border colour    |
| `section_title_color`   | Section title colour     |
| `tool_title_color`      | Tool title colour        |
| `tool_value_color`      | Tool value colour        |
| `tool_label_color`      | Tool label colour        |
| `tool_running_accent`   | Tool mid-execution       |
| `tool_success_accent`   | Tool finished OK         |
| `tool_failed_accent`    | Tool failed              |

### Plan Colours

| Field                   | Description       |
|-------------------------|-------------------|
| `plan_progress_color`   | Progress bar      |
| `plan_summary_color`    | Summary text      |
| `plan_explanation_color`| Explanation text  |
| `plan_pending_color`    | Pending step      |
| `plan_in_progress_color`| In-progress step  |
| `plan_completed_color`  | Completed step    |

### Reasoning

| Field          | Description                                                     |
|----------------|-----------------------------------------------------------------|
| `reasoning_bg` | Thinking-block background tint (`#RRGGBB` or `"reset"` to clear) |

## Activation

### From the Theme Picker

Type `/theme` in the TUI.  The picker lists all built-in presets plus any
custom theme files found in `~/.config/deepseek/themes/`.  Custom entries
appear with a ***(custom)*** suffix.

### From the Settings Editor

```
/config theme file:mo
```

The `file:` prefix tells the engine to load the custom theme file `mo.toml`.
Without the prefix the value is treated as a built-in preset name.

### Persistent in Config

```toml
theme = "file:midnight"
```

in `~/.deepseek/config.toml` under the `[settings]` section.  You can also set
individual overrides directly in settings:

```toml
[settings]
sidebar_bg  = "#1A1B26"
border_type = "rounded"
```

Settings-level overrides are applied on top of whatever theme is active
(built-in or custom).

## Inheritance Rules

1. Start with the base theme's full palette.
2. For every field present in the custom theme file, replace the base value.
3. Theme-file values can be further refined at runtime via `/config` settings
   (`sidebar_bg`, `composer_bg`, `border_type`, `section_border_type`,
   `background_color`).
4. The `"reset"` keyword clears a field back to its base-theme default.

## Example: Midnight Blue

`~/.config/deepseek/themes/midnight.toml`:

```toml
base = "dark"

background_color = "#0A0B14"
panel_bg         = "#0F1020"
sidebar_bg       = "#0F1020"
composer_bg      = "#0F1020"
border_type      = "rounded"
border_color     = "#1E2040"
reasoning_bg     = "#151630"
```

Activate with `/theme midnight` or `theme = "file:midnight"`.

## Reference

- `theme.example.toml` — fully-annotated template shipped in the repo root.
- [CONFIGURATION.md](CONFIGURATION.md) — `theme`, `sidebar_bg`, `composer_bg`,
  `border_type`, `section_border_type`, and `background_color` settings.
