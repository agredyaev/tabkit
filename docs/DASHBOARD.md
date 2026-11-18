# Native dashboard authoring

This document covers `create_dashboard`. For single-extension HTML/CSS/JavaScript dashboards, see [Web dashboard extension](WEB_DASHBOARD.md).

TabKit native dashboard authoring is a semantic layout compiler. The caller specifies layout, sizing,
styles, worksheet references, filter cards, and selected worksheet formatting. TabKit owns Tableau
zone IDs, normalized coordinates, dashboard dependencies, dashboard windows/viewpoints, and XML
ordering.

## Example

```json
{
  "edits": [
    {
      "op": "create_dashboard",
      "name": "Executive Overview",
      "width": 1440,
      "height": 900,
      "padding": 24,
      "background": "#F5F6F8",
      "view_styles": [
        {
          "sheet": "Sales Trend",
          "font_family": "Tableau Book",
          "font_size": 11,
          "color": "#333333",
          "axis": {
            "visible": true,
            "color": "#DADCE0",
            "width": 1
          },
          "grid_lines": {
            "visible": true,
            "color": "#ECEFF1",
            "width": 1
          },
          "zero_line": {
            "visible": false,
            "color": null,
            "width": null
          },
          "line_width": 2.0,
          "show_labels": false
        }
      ],
      "layout": {
        "type": "column",
        "gap": 16,
        "padding": 0,
        "children": [
          {
            "fixed_size": 56,
            "style": {},
            "type": "text",
            "text": "Executive Overview",
            "font_family": "Tableau Book",
            "font_size": 24,
            "bold": true,
            "italic": false,
            "color": "#202124",
            "align": "left"
          },
          {
            "weight": 1,
            "style": {},
            "type": "row",
            "gap": 16,
            "padding": 8,
            "children": [
              {
                "weight": 1,
                "style": {
                  "background": "#FFFFFF",
                  "border_color": "#E4E6EA",
                  "border_width": 1,
                  "corner_radius": 8,
                  "padding": 8,
                  "margin": 0
                },
                "type": "sheet",
                "sheet": "Sales Trend",
                "show_title": false,
                "fit": "fit_width"
              },
              {
                "fixed_size": 280,
                "style": {
                  "background": "#FFFFFF",
                  "border_color": "#E4E6EA",
                  "border_width": 1,
                  "corner_radius": 8,
                  "padding": 8,
                  "margin": 0
                },
                "type": "column",
                "gap": 12,
                "padding": 8,
                "children": [
                  {
                    "fixed_size": 72,
                    "style": {},
                    "type": "filter",
                    "source_sheet": "Sales Trend",
                    "field": "Region",
                    "instance": null,
                    "control": "multiple_dropdown",
                    "values": "relevant",
                    "title": "Region"
                  },
                  {
                    "weight": 1,
                    "style": {},
                    "type": "spacer"
                  }
                ]
              }
            ]
          }
        ]
      }
    }
  ]
}
```

## Layout model

Rows split horizontal space and columns split vertical space.

Each child can use one of two sizing modes:

- `fixed_size`: pixels along the parent axis.
- `weight`: relative share of the remaining space after fixed children and gaps.

Containers support:

- `gap`: pixels between children.
- `padding`: pixels inside the container before child layout.
- zone styling through the containing `Item.style`.

The compiler converts pixel layout into Tableau's normalized 100000 x 100000 dashboard coordinate
space. A fixed child also emits Tableau `fixed-size` / `is-fixed` metadata.

## Zone styles

`style` supports:

- background
- border color
- border width
- corner radius
- padding
- margin

Colors use `#RRGGBB`.

## Worksheet views

A sheet zone supports:

- `show_title`
- `fit`: `standard`, `entire_view`, `fit_width`, or `fit_height`

Fit is written to the dashboard window/viewpoint.

## Text

Text zones support:

- font family
- font size
- bold
- italic
- color
- left/center/right alignment

## Worksheet formatting

`view_styles` is keyed by worksheet name and supports:

- worksheet font family / size / color
- axis visibility/color/width
- grid line visibility/color/width
- zero line visibility/color/width
- mark line/size value
- mark label visibility

TabKit emits worksheet/table/pane style rules as source-preserving patches.

## Filter cards

A dashboard filter is a layout object:

```json
{
  "type": "filter",
  "source_sheet": "Sales Trend",
  "field": "Region",
  "instance": null,
  "control": "multiple_dropdown",
  "values": "relevant",
  "title": "Region"
}
```

Supported control values:

- `auto` (default; preserve Tableau-native card behavior)
- `single_dropdown`
- `multiple_dropdown`
- `single_list`
- `multiple_list`
- `slider`

Supported value domains (optional; omit to preserve the native card default):

- `all_database` -> Tableau `values="database"`
- `relevant` -> Tableau `values="relevant"`

The source worksheet must already contain the field on its Filters shelf and expose a matching
`column-instance`. TabKit then:

1. resolves the source worksheet and field through the typed workbook index;
2. resolves the exact Tableau column-instance;
3. verifies that the worksheet already has the filter;
4. copies the required datasource dependency metadata into the dashboard;
5. emits the dashboard filter zone at the requested position.

If a worksheet has more than one column-instance for the same field, pass the exact `instance`.
TabKit does not guess.

### Filter and action boundary

A filter card controls its source worksheet's existing Filters-shelf filter. TabKit does not synthesize
Tableau's persisted `Apply to Worksheets` sharing state.

Cross-sheet interaction is available through `create_filter_action`. The source worksheet must
already be placed on the dashboard; selecting marks on it filters the other compatible worksheets on
that dashboard. The source worksheet is excluded from its own action.

Parameter-driven interaction is available through `create_parameter_action`. Parameters and actions
are documented in [AUTHORING.md](AUTHORING.md).
