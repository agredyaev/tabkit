# Native dashboards

`create_dashboard` compiles a semantic layout tree into a fixed-size Tableau dashboard.
The caller controls layout and supported presentation properties; TabKit owns Tableau zone IDs,
normalized coordinates, dependencies, windows/viewpoints, and XML ordering.

## Supported surface

| Area | Supported |
| --- | --- |
| layout | nested rows / columns, fixed size, weight, gap, padding |
| content | worksheet, text, filter card, spacer |
| zone style | background, border, corner radius, padding, margin |
| worksheet view | title visibility, fit mode |
| worksheet style | font, axes, grid/zero lines, line width, labels |
| filters | existing worksheet Filters-shelf filters |
| interaction | filter and parameter actions are authored separately |

## Layout model

Rows split horizontal space; columns split vertical space.

| Property | Meaning |
| --- | --- |
| `fixed_size` | pixels on the parent axis |
| `weight` | share of remaining space |
| `gap` | pixels between children |
| `padding` | pixels inside a container |

TabKit converts pixel layout to Tableau's 100000 x 100000 dashboard coordinate space.

| `type` | Key fields |
| --- | --- |
| `row`, `column` | `children`, `gap`, `padding` |
| `sheet` | `sheet`, `show_title`, `fit` |
| `filter` | `source_sheet`, `field`, optional `instance`, `control`, `values` |
| `text` | text + font/alignment options |
| `spacer` | no content |

Fit modes: `standard`, `entire_view`, `fit_width`, `fit_height`.

## Example

```json
{
  "op": "create_dashboard",
  "name": "Executive Overview",
  "width": 1440,
  "height": 900,
  "padding": 24,
  "background": "#F5F6F8",
  "layout": {
    "type": "column",
    "gap": 16,
    "children": [
      {
        "fixed_size": 56,
        "type": "text",
        "text": "Executive Overview",
        "font_size": 24,
        "bold": true
      },
      {
        "weight": 1,
        "type": "row",
        "gap": 16,
        "children": [
          { "weight": 1, "type": "sheet", "sheet": "Sales Trend", "fit": "fit_width" },
          {
            "fixed_size": 280,
            "type": "filter",
            "source_sheet": "Sales Trend",
            "field": "Region",
            "control": "multiple_dropdown",
            "values": "relevant"
          }
        ]
      }
    ]
  }
}
```

## Styles

Each layout item can carry:

`background` · `border_color` · `border_width` · `corner_radius` · `padding` · `margin`

Colors use `#RRGGBB`.

`view_styles` applies worksheet formatting by worksheet name:

`font_family` · `font_size` · `color` · `axis` · `grid_lines` · `zero_line` · `line_width` · `show_labels`

## Filter cards

A filter card exposes a filter that already exists on the source worksheet.

Controls:

`auto` · `single_dropdown` · `multiple_dropdown` · `single_list` · `multiple_list` · `slider`

Value domains:

`all_database` · `relevant`

Resolution rules:

1. source worksheet must exist;
2. field must already be on that worksheet's Filters shelf;
3. TabKit resolves the matching Tableau `column-instance`;
4. if more than one instance matches, the caller must provide `instance`;
5. required datasource-dependency metadata is copied into the dashboard.

A filter card does not create Tableau's persisted **Apply to Worksheets** sharing state.
Use `create_filter_action` for cross-sheet filtering.

## Boundaries

`create_dashboard` does not create worksheets, fields, calculations, or parameters implicitly.
Dependencies must already exist on the source handle used for the edit.

For HTML/CSS/JavaScript presentation, use [Web dashboards](WEB_DASHBOARD.md).
