#![allow(clippy::expect_used)]

#[cfg(test)]
mod tests {
    use crate::{
        edit::{
            self, Batch, CalculationSpec, DashboardSpec, Edit, FilterControl, Item, Layout,
            ZoneStyle,
        },
        lineage::{Direction, Kind, Lineage},
        workbook::{self, DataType, FieldType, Role, Workbook},
    };
    use serde_json::json;
    use std::{
        hint::black_box,
        time::{Duration, Instant},
    };

    struct Case {
        target_mib: usize,
        fields: usize,
        worksheets: usize,
        parse_iterations: usize,
        lineage_iterations: usize,
        inspect_iterations: usize,
        edit_iterations: usize,
    }

    fn synthetic(case: &Case) -> String {
        let mut xml = String::with_capacity(case.target_mib * 1024 * 1024 + 4096);
        xml.push_str(
            r#"<workbook version="26.2" source-build="20262.benchmark">
  <datasources>
    <datasource inline="true" name="Orders" version="26.2">
      <connection class="postgres" name="postgres.1" one-time-sql="SELECT * FROM bootstrap.tenant">
        <relation connection="postgres.1" name="Orders SQL" type="text">SELECT * FROM warehouse.orders</relation>
      </connection>
"#,
        );

        for i in 0..case.fields {
            if i % 20 == 19 {
                xml.push_str(&format!(
                    "      <column caption=\"Calc {i:06}\" datatype=\"real\" name=\"[C{i:06}]\" role=\"measure\" type=\"quantitative\"><calculation class=\"tableau\" formula=\"[F000000] + [F000001]\"/></column>\n"
                ));
            } else {
                xml.push_str(&format!(
                    "      <column caption=\"Field {i:06}\" datatype=\"real\" name=\"[F{i:06}]\" role=\"measure\" type=\"quantitative\"/>\n"
                ));
            }
        }
        xml.push_str("    </datasource>\n  </datasources>\n  <worksheets>\n");

        xml.push_str(
            r#"    <worksheet name="Sheet A">
      <table><view><datasource-dependencies datasource="Orders">
        <column datatype="real" name="[F000000]" role="measure" type="quantitative"/>
      </datasource-dependencies></view></table>
      <simple-id uuid="{00000000-0000-0000-0000-000000000001}"/>
    </worksheet>
    <worksheet name="Sheet B">
      <table><view><datasource-dependencies datasource="Orders">
        <column datatype="real" name="[F000001]" role="measure" type="quantitative"/>
      </datasource-dependencies></view></table>
      <simple-id uuid="{00000000-0000-0000-0000-000000000002}"/>
    </worksheet>
"#,
        );

        for i in 0..case.worksheets {
            let field = i % case.fields.max(1);
            xml.push_str(&format!(
                "    <worksheet name=\"Bench Sheet {i:05}\"><table><view><datasource-dependencies datasource=\"Orders\"><column datatype=\"real\" name=\"[F{field:06}]\" role=\"measure\" type=\"quantitative\"/></datasource-dependencies></view></table><simple-id uuid=\"{{00000000-0000-0000-0001-{i:012}}}\"/></worksheet>\n"
            ));
        }
        xml.push_str("  </worksheets>\n");

        let target = case.target_mib * 1024 * 1024;
        let closing = "</workbook>\n";
        if xml.len() + closing.len() + 8 < target {
            let filler = target - xml.len() - closing.len() - 7;
            xml.push_str("<!--");
            xml.extend(std::iter::repeat_n('x', filler));
            xml.push_str("-->\n");
        }
        xml.push_str(closing);
        xml
    }

    fn batch() -> Batch {
        Batch {
            edits: vec![
                Edit::CreateCalculation(CalculationSpec {
                    datasource: "Orders".into(),
                    caption: "Benchmark Calc".into(),
                    formula: "[F000000] / [F000001]".into(),
                    datatype: DataType::Real,
                    role: Role::Measure,
                    field_type: FieldType::Quantitative,
                    name: None,
                }),
                Edit::CreateDashboard(DashboardSpec {
                    name: "Benchmark Dashboard".into(),
                    width: 1440,
                    height: 900,
                    padding: 24,
                    background: Some("#F7F7F7".into()),
                    view_styles: Vec::new(),
                    layout: Layout::Row {
                        gap: 16,
                        padding: 0,
                        children: vec![
                            Item {
                                weight: 2,
                                fixed_size: None,
                                style: ZoneStyle::default(),
                                layout: Layout::Sheet {
                                    sheet: "Sheet A".into(),
                                    show_title: None,
                                    fit: None,
                                },
                            },
                            Item {
                                weight: 1,
                                fixed_size: None,
                                style: ZoneStyle::default(),
                                layout: Layout::Sheet {
                                    sheet: "Sheet B".into(),
                                    show_title: None,
                                    fit: None,
                                },
                            },
                        ],
                    },
                }),
            ],
        }
    }

    fn filter_workbook(dependency_fields: usize) -> Workbook {
        let mut xml = String::with_capacity(dependency_fields.saturating_mul(180) + 4096);
        xml.push_str(
            r#"<workbook version="26.2" source-build="20262.filter-benchmark">
  <datasources>
    <datasource inline="true" name="Orders" version="26.2">
"#,
        );
        for index in 0..dependency_fields {
            xml.push_str(&format!(
                "      <column datatype=\"string\" name=\"[F{index:06}]\" role=\"dimension\" type=\"nominal\"/>\n"
            ));
        }
        xml.push_str(
            r#"    </datasource>
  </datasources>
  <worksheets>
    <worksheet name="Filter Sheet">
      <table><view><datasource-dependencies datasource="Orders">
"#,
        );
        for index in 0..dependency_fields {
            xml.push_str(&format!(
                "        <column datatype=\"string\" name=\"[F{index:06}]\" role=\"dimension\" type=\"nominal\"/>\n"
            ));
        }
        xml.push_str(
            r#"        <column-instance column="[F000000]" derivation="None" name="[none:F000000:nk]" pivot="key" type="nominal"/>
      </datasource-dependencies>
      <filter class="categorical" column="[Orders].[none:F000000:nk]"/>
      </view></table>
      <simple-id uuid="{00000000-0000-0000-0000-000000000001}"/>
    </worksheet>
  </worksheets>
</workbook>
"#,
        );
        Workbook::open(xml.into_bytes()).expect("filter benchmark workbook")
    }

    fn filter_batch(count: usize) -> Batch {
        Batch {
            edits: vec![Edit::CreateDashboard(DashboardSpec {
                name: format!("Filter Benchmark {count}"),
                width: 1200,
                height: 900,
                padding: 12,
                background: None,
                view_styles: Vec::new(),
                layout: Layout::Column {
                    gap: 0,
                    padding: 0,
                    children: (0..count)
                        .map(|index| Item {
                            weight: 1,
                            fixed_size: None,
                            style: ZoneStyle::default(),
                            layout: Layout::Filter {
                                source_sheet: "Filter Sheet".into(),
                                field: "F000000".into(),
                                instance: Some("[none:F000000:nk]".into()),
                                control: FilterControl::Auto,
                                values: None,
                                title: Some(format!("Filter {index:03}")),
                            },
                        })
                        .collect(),
                },
            })],
        }
    }

    fn calculation_batch(count: usize) -> Batch {
        Batch {
            edits: (0..count)
                .map(|index| {
                    Edit::CreateCalculation(CalculationSpec {
                        datasource: "Orders".into(),
                        caption: format!("Bulk Calc {index:03}"),
                        formula: "[F000000] + [F000001]".into(),
                        datatype: DataType::Real,
                        role: Role::Measure,
                        field_type: FieldType::Quantitative,
                        name: Some(format!("[Bulk Calc {index:03}]")),
                    })
                })
                .collect(),
        }
    }

    fn measure(iterations: usize, mut f: impl FnMut()) -> Vec<Duration> {
        let mut samples = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let start = Instant::now();
            f();
            samples.push(start.elapsed());
        }
        samples
    }

    fn stats(samples: &[Duration], divisor: f64) -> serde_json::Value {
        let mut values: Vec<f64> = samples
            .iter()
            .map(|sample| sample.as_secs_f64() * divisor)
            .collect();
        values.sort_by(f64::total_cmp);
        let percentile = |p: f64| {
            let index = ((values.len().saturating_sub(1)) as f64 * p).round() as usize;
            values[index]
        };
        json!({
            "median": percentile(0.50),
            "p95": percentile(0.95),
            "min": values[0],
            "max": values[values.len() - 1]
        })
    }

    #[test]
    #[ignore = "release-only performance/profile harness"]
    fn runtime_profile() {
        let cases = [
            Case {
                target_mib: 1,
                fields: 2_000,
                worksheets: 20,
                parse_iterations: 12,
                lineage_iterations: 12,
                inspect_iterations: 500,
                edit_iterations: 5,
            },
            Case {
                target_mib: 10,
                fields: 20_000,
                worksheets: 200,
                parse_iterations: 6,
                lineage_iterations: 6,
                inspect_iterations: 200,
                edit_iterations: 3,
            },
            Case {
                target_mib: 50,
                fields: 80_000,
                worksheets: 800,
                parse_iterations: 3,
                lineage_iterations: 3,
                inspect_iterations: 100,
                edit_iterations: 1,
            },
        ];

        let mut results = Vec::new();
        for case in cases {
            let source = synthetic(&case);
            let bytes = source.len();
            let mib = bytes as f64 / (1024.0 * 1024.0);

            let parse_samples = measure(case.parse_iterations, || {
                let input = source.as_bytes().to_vec();
                black_box(Workbook::open(input).expect("parse/index"));
            });

            let workbook =
                Workbook::open(source.as_bytes().to_vec()).expect("benchmark workbook parse");
            let summary = workbook.summary();

            let lineage_samples = measure(case.lineage_iterations, || {
                black_box(Lineage::build(&workbook).expect("lineage"));
            });
            let lineage = Lineage::build(&workbook).expect("trace lineage");
            let find_start = Instant::now();
            for _ in 0..case.inspect_iterations {
                black_box(lineage.find_kind("Field 000000", Some(Kind::Field)));
            }
            let exact_find_each_us =
                find_start.elapsed().as_secs_f64() * 1_000_000.0 / case.inspect_iterations as f64;
            let trace_root = lineage
                .find_kind("Field 000000", Some(Kind::Field))
                .first()
                .expect("trace root")
                .id;
            let trace_start = Instant::now();
            for _ in 0..case.inspect_iterations {
                black_box(
                    lineage
                        .trace_limited(trace_root, Direction::Downstream, 1_000)
                        .expect("trace"),
                );
            }
            let trace_each_us =
                trace_start.elapsed().as_secs_f64() * 1_000_000.0 / case.inspect_iterations as f64;

            let inspect_start = Instant::now();
            for _ in 0..case.inspect_iterations {
                black_box(
                    workbook::inspect(&workbook, "field", Some("Field 000000")).expect("inspect"),
                );
            }
            let inspect_each_us = inspect_start.elapsed().as_secs_f64() * 1_000_000.0
                / case.inspect_iterations as f64;

            let edit_batch = batch();
            let edit_samples = measure(case.edit_iterations, || {
                black_box(
                    edit::apply(&workbook, &edit_batch, (bytes as u64).saturating_mul(2))
                        .expect("edit"),
                );
            });
            let bulk_calculations = calculation_batch(100);
            let bulk_calculation_samples = measure(case.edit_iterations, || {
                black_box(
                    edit::apply(
                        &workbook,
                        &bulk_calculations,
                        (bytes as u64).saturating_mul(2),
                    )
                    .expect("bulk calculation edit"),
                );
            });

            let parse_median_s = {
                let mut sorted = parse_samples.clone();
                sorted.sort();
                sorted[sorted.len() / 2].as_secs_f64()
            };

            results.push(json!({
                "input_mib": mib,
                "xml_bytes": bytes,
                "objects": {
                    "fields": summary.fields,
                    "calculations": summary.calculations,
                    "worksheets": summary.worksheets,
                    "dashboards": summary.dashboards
                },
                "parse_index_ms": stats(&parse_samples, 1000.0),
                "parse_throughput_mib_s": mib / parse_median_s,
                "lineage_ms": stats(&lineage_samples, 1000.0),
                "lineage_exact_find_us": exact_find_each_us,
                "lineage_trace_1000_us": trace_each_us,
                "exact_field_inspect_us": inspect_each_us,
                "edit_calc_dashboard_ms": stats(&edit_samples, 1000.0),
                "edit_100_calculations_ms": stats(&bulk_calculation_samples, 1000.0)
            }));
        }

        let filter_workbook = filter_workbook(20_000);
        let filter_input_mib = filter_workbook.xml.source().len() as f64 / (1024.0 * 1024.0);
        let mut filter_results = Vec::new();
        for cards in [1usize, 10, 100] {
            let batch = filter_batch(cards);
            let samples = measure(3, || {
                black_box(
                    edit::apply(
                        &filter_workbook,
                        &batch,
                        (filter_workbook.xml.source().len() as u64).saturating_mul(2),
                    )
                    .expect("filter dashboard edit"),
                );
            });
            filter_results.push(json!({
                "cards": cards,
                "edit_ms": stats(&samples, 1000.0)
            }));
        }

        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "mode": "release",
                "cases": results,
                "filter_scaling": {
                    "dependency_fields": 20_000,
                    "input_mib": filter_input_mib,
                    "cases": filter_results
                }
            }))
            .expect("benchmark JSON")
        );
    }
}
