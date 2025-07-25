use crate::core::color::SuzakuColor;
use crate::core::color::SuzakuColor::{Cyan, Green, Orange, Red, White, Yellow};
use crate::core::rules;
use crate::core::scan::{scan_directory, scan_file};
use crate::core::util::{get_json_writer, get_writer, output_path_info, p};
use crate::option::cli::{AwsCtTimelineOptions, CommonOptions};
use crate::option::geoip::GeoIPSearch;
use chrono::{DateTime, Utc};
use comfy_table::modifiers::UTF8_ROUND_CORNERS;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Cell, Table, TableComponent};
use csv::Writer;
use krapslog::{build_sparkline, build_time_markers};
use num_format::{Locale, ToFormattedString};
use serde_json::Value;
use sigma_rust::{Event, Rule};
use std::cmp::min;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::io::{BufWriter, Write};
use termcolor::{BufferWriter, Color, ColorChoice, ColorSpec, WriteColor};
use terminal_size::{Width, terminal_size};

#[derive(Debug, Default)]
pub struct DetectionSummary {
    pub author_titles: HashMap<String, HashSet<String>>,
    pub timestamps: Vec<i64>,
    pub total_events: usize,
    pub event_with_hits: usize,
    pub dates_with_hits: HashMap<String, HashMap<String, usize>>,
    pub level_with_hits: HashMap<String, HashMap<String, usize>>,
    pub first_event_time: Option<DateTime<Utc>>,
    pub last_event_time: Option<DateTime<Utc>>,
}

#[derive(Debug)]
pub enum OutputType {
    Csv,
    Json,
    Jsonl,
    CsvAndJson,
    CsvAndJsonl,
}

impl OutputType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(OutputType::Csv),
            2 => Some(OutputType::Json),
            3 => Some(OutputType::Jsonl),
            4 => Some(OutputType::CsvAndJson),
            5 => Some(OutputType::CsvAndJsonl),
            _ => None,
        }
    }
}

pub struct Writers {
    csv: Option<Writer<Box<dyn Write>>>,
    json: Option<BufWriter<Box<dyn Write>>>,
    jsonl: Option<BufWriter<Box<dyn Write>>>,
    std: Option<BufferWriter>,
}

#[allow(clippy::too_many_arguments)]
pub fn write_record(
    profile: &[(String, String)],
    event: &Event,
    json: &Value,
    rule: &Rule,
    wrt: &mut Writers,
    no_color: bool,
    geo: &mut Option<GeoIPSearch>,
    raw_output: bool,
) {
    let mut record: Vec<String> = profile
        .iter()
        .map(|(_k, v)| get_value_from_event(v, event, rule, geo))
        .collect();

    // 標準出力
    if let Some(writer) = &mut wrt.std {
        let level_index = profile.iter().position(|(k, _)| k == "Level");
        let level = if let Some(index) = level_index {
            let org = record[index].to_lowercase();
            let abb = abbreviate_level(&org);
            record[index] = abb.to_string();
            abb.to_string()
        } else {
            "info".to_string()
        };
        let color = if level == "crit" {
            Red
        } else if level == "high" {
            Orange
        } else if level == "med" {
            Yellow
        } else if level == "low" {
            Green
        } else {
            White
        };

        let mut buf = writer.buffer();
        for (i, col) in record.iter().enumerate() {
            buf.set_color(ColorSpec::new().set_fg(color.rdg(no_color)))
                .ok();
            write!(buf, "{}", col).ok();
            if i != record.len() - 1 {
                if no_color {
                    buf.set_color(ColorSpec::new().set_fg(None)).ok();
                } else {
                    buf.set_color(ColorSpec::new().set_fg(Orange.rdg(no_color)))
                        .ok();
                }
                write!(buf, " · ").ok();
            }
        }
        write!(buf, "\n\n").ok();
        writer.print(&buf).ok();
    }

    // CSV出力
    if let Some(writer) = &mut wrt.csv {
        writer.write_record(&record).unwrap();
    }

    // JSON出力
    if let Some(writer) = &mut wrt.json {
        if raw_output {
            let mut json_record = json.clone();
            let sigma_profile: Vec<(String, String)> = profile
                .iter()
                .filter(|(_, value)| value.starts_with("sigma."))
                .cloned()
                .collect();
            for (k, v) in sigma_profile {
                let value = get_value_from_event(&v, event, rule, geo);
                json_record[k] = Value::String(value.to_string());
            }
            let rec = serde_json::to_string_pretty(&json_record);
            if let Ok(json_string) = rec {
                writer.write_all(json_string.as_bytes()).unwrap();
                writer.write_all(b"\n").unwrap();
            }
            return;
        }
        let mut json_record: BTreeMap<String, String> = BTreeMap::new();
        for (k, v) in profile {
            let value = get_value_from_event(v, event, rule, geo);
            json_record.insert(k.clone(), value.to_string());
        }
        let rec = serde_json::to_string_pretty(&json_record);
        if let Ok(json_string) = rec {
            writer.write_all(json_string.as_bytes()).unwrap();
            writer.write_all(b"\n").unwrap();
        }
    }

    // JSONL出力
    if let Some(writer) = &mut wrt.jsonl {
        if raw_output {
            let mut json_record = json.clone();
            let sigma_profile: Vec<(String, String)> = profile
                .iter()
                .filter(|(_, value)| value.starts_with("sigma."))
                .cloned()
                .collect();
            for (k, v) in sigma_profile {
                let value = get_value_from_event(&v, event, rule, geo);
                json_record[k] = Value::String(value.to_string());
            }
            let rec = serde_json::to_string(&json_record);
            if let Ok(json_string) = rec {
                writer.write_all(json_string.as_bytes()).unwrap();
                writer.write_all(b"\n").unwrap();
            }
            return;
        }
        let mut json_record: BTreeMap<String, String> = BTreeMap::new();
        for (k, v) in profile {
            let value = get_value_from_event(v, event, rule, geo);
            json_record.insert(k.clone(), value.to_string());
        }
        if let Ok(json_string) = serde_json::to_string(&json_record) {
            writer.write_all(json_string.as_bytes()).unwrap();
            writer.write_all(b"\n").unwrap();
        }
    }
}

fn abbreviate_level(level: &str) -> &str {
    match level {
        "critical" => "crit",
        "medium" => "med",
        "informational" => "info",
        _ => level,
    }
}

pub fn aws_detect(options: &AwsCtTimelineOptions, common_opt: &CommonOptions) {
    let no_color = common_opt.no_color;
    let mut geo_search = None;
    if let Some(path) = options.geo_ip.as_ref() {
        let res = GeoIPSearch::new(path);
        if let Ok(geo) = res {
            geo_search = Some(geo);
        } else {
            p(
                Red.rdg(no_color),
                "Could not find the appropriate MaxMind GeoIP .mmdb database files.\n",
                true,
            );
            return;
        }
    }
    let profile = load_profile("config/default_profile.yaml", &geo_search);
    let rules: Vec<Rule> = rules::load_rules_from_dir(&options.rules);
    if rules.is_empty() {
        p(
            Red.rdg(no_color),
            "Suzaku could not load any rules. Please download the rules with the update-rules command.\n",
            true,
        );
        return;
    }
    let rules = rules::filter_rules_by_level(&rules, &options.min_level);

    p(Green.rdg(no_color), "Total detection rules: ", false);
    p(None, rules.len().to_string().as_str(), true);

    let mut std_writer = None;
    let mut csv_writer = None;
    let mut json_writer = None;
    let mut jsonl_writer = None;
    let mut output_pathes = vec![];

    if let Some(output_path) = &options.output {
        let output_type = OutputType::from_u8(options.output_type).unwrap_or(OutputType::Csv);
        match output_type {
            OutputType::Csv | OutputType::CsvAndJson | OutputType::CsvAndJsonl => {
                let mut csv_path = output_path.clone();
                if csv_path.extension().and_then(|ext| ext.to_str()) != Some("csv") {
                    csv_path.set_extension("csv");
                }
                output_pathes.push(csv_path.clone());
                csv_writer = Some(get_writer(&Some(csv_path)));
            }
            _ => {}
        }
        match output_type {
            OutputType::Json | OutputType::CsvAndJson => {
                let mut json_path = output_path.clone();
                if json_path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    json_path.set_extension("json");
                }
                output_pathes.push(json_path.clone());
                json_writer = Some(get_json_writer(&Some(json_path)));
            }
            OutputType::Jsonl | OutputType::CsvAndJsonl => {
                let mut jsonl_path = output_path.clone();
                if jsonl_path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                    jsonl_path.set_extension("jsonl");
                }
                output_pathes.push(jsonl_path.clone());
                jsonl_writer = Some(get_json_writer(&Some(jsonl_path)));
            }
            _ => {}
        }
    } else {
        let disp_wtr = BufferWriter::stdout(ColorChoice::Always);
        let mut disp_wtr_buf = disp_wtr.buffer();
        disp_wtr_buf.set_color(ColorSpec::new().set_fg(None)).ok();
        std_writer = Some(disp_wtr);
    }

    if let Some(ref mut std_out) = std_writer {
        let csv_header: Vec<&str> = profile.iter().map(|(k, _v)| k.as_str()).collect();
        let mut buf = std_out.buffer();
        writeln!(buf, "{}", csv_header.join(" · ")).ok();
    }

    if let Some(ref mut writer) = csv_writer {
        let csv_header: Vec<&str> = profile.iter().map(|(k, _v)| k.as_str()).collect();
        writer.write_record(&csv_header).unwrap();
    }
    let mut wrt = Writers {
        csv: csv_writer,
        json: json_writer,
        jsonl: jsonl_writer,
        std: std_writer,
    };

    let mut summary = DetectionSummary::default();
    if let Some(d) = &options.input_opt.directory {
        scan_directory(
            d,
            options,
            &rules,
            &mut summary,
            &profile,
            &mut wrt,
            common_opt,
            &mut geo_search,
        );
    } else if let Some(f) = &options.input_opt.filepath {
        scan_file(
            f,
            options,
            &rules,
            &mut summary,
            &profile,
            &mut wrt,
            common_opt,
            &mut geo_search,
        );
    }
    if let Some(ref mut writer) = wrt.csv {
        writer.flush().unwrap();
    }
    if let Some(ref mut writer) = wrt.json {
        writer.flush().unwrap();
    }
    if let Some(ref mut writer) = wrt.jsonl {
        writer.flush().unwrap();
    }
    println!();
    let terminal_width = match terminal_size() {
        Some((Width(w), _)) => w as usize,
        None => 100,
    };
    if !options.no_frequency {
        print_timeline_hist(&summary.timestamps, terminal_width, 3);
    }
    let table_column_num = if terminal_width <= 105 {
        2
    } else if terminal_width < 140 {
        3
    } else if terminal_width < 175 {
        4
    } else if terminal_width <= 210 {
        5
    } else {
        6
    };

    let authors_count: HashMap<String, i128> = summary
        .author_titles
        .iter()
        .map(|(author, rules)| (author.clone(), rules.len() as i128))
        .collect();

    print_detected_rule_authors(&authors_count, table_column_num, no_color);

    if !options.no_summary {
        print_summary(&summary, no_color);
    }

    if !output_pathes.is_empty() {
        output_path_info(no_color, &output_pathes);
    }
}

fn print_summary(sum: &DetectionSummary, no_color: bool) {
    let levels = if no_color {
        vec![
            ("critical", White),
            ("high", White),
            ("medium", White),
            ("low", White),
            ("informational", White),
        ]
    } else {
        vec![
            ("critical", Red),
            ("high", Orange),
            ("medium", Yellow),
            ("low", Green),
            ("informational", White),
        ]
    };
    print_summary_header(sum, no_color);
    print_summary_levels(sum, &levels);
    print_summary_event_times(sum);
    print_summary_dates_with_hits(sum, &levels);
    print_summary_table(sum, &levels);
}

fn print_summary_header(sum: &DetectionSummary, no_color: bool) {
    p(Green.rdg(no_color), "Results Summary:", true);
    p(None, "", false);
    p(Green.rdg(no_color), "Events with hits", false);
    p(None, " / ", false);
    p(Green.rdg(no_color), "Total events: ", false);
    let msg = sum.event_with_hits.to_formatted_string(&Locale::en);
    p(Yellow.rdg(no_color), msg.as_str(), false);
    p(None, " / ", false);
    let msg = sum.total_events.to_formatted_string(&Locale::en);
    p(Cyan.rdg(no_color), msg.as_str(), false);
    p(None, " (", false);
    p(
        Green.rdg(no_color),
        &format!(
            "Data reduction: {} events ({:.2}%)",
            (sum.total_events - sum.event_with_hits).to_formatted_string(&Locale::en),
            (sum.total_events - sum.event_with_hits) as f64 * 100.0 / sum.total_events as f64
        ),
        false,
    );
    p(None, ")", false);
    println!();
}

fn print_summary_levels(sum: &DetectionSummary, levels: &Vec<(&str, SuzakuColor)>) {
    for (level, color) in levels {
        if let Some(hits) = sum.level_with_hits.get(*level) {
            let uniq_hits = hits.keys().len();
            let total_hits: usize = hits.values().sum();
            let msg = format!(
                "Total | Unique {} detections: {} ({:.2}%) | {} ({:.2}%)",
                level,
                total_hits.to_formatted_string(&Locale::en),
                total_hits * 100 / sum.event_with_hits,
                uniq_hits,
                uniq_hits * 100 / sum.event_with_hits
            );
            p(color.rdg(false), &msg, true);
        } else {
            let msg = format!("Total | Unique {} detections: 0 (0%) | 0 (0%)", level);
            p(color.rdg(false), &msg, true);
        }
    }
    println!();
}

fn print_summary_event_times(sum: &DetectionSummary) {
    if let Some(first_event_time) = sum.first_event_time {
        p(None, "First event time: ", false);
        p(None, &first_event_time.to_string(), true);
    }
    if let Some(last_event_time) = sum.last_event_time {
        p(None, "Last event time: ", false);
        p(None, &last_event_time.to_string(), true);
    }
    println!();
}

fn print_summary_dates_with_hits(sum: &DetectionSummary, levels: &Vec<(&str, SuzakuColor)>) {
    p(None, "Dates with most total detections:", true);
    for (level, color) in levels {
        if let Some(dates) = sum.dates_with_hits.get(*level) {
            if let Some((date, &max_hits)) = dates.iter().max_by_key(|&(_, &count)| count) {
                let msg = format!(
                    "{}: {} ({})",
                    level,
                    date,
                    max_hits.to_formatted_string(&Locale::en)
                );
                p(color.rdg(false), &msg, false);
            }
        } else {
            p(color.rdg(false), &format!("{}: n/a", level), false);
        }
        if *level != "informational" {
            p(None, ", ", false);
        }
    }
    println!();
}

fn print_summary_table(sum: &DetectionSummary, levels: &Vec<(&str, SuzakuColor)>) {
    let mut table_data = vec![];
    for (level, color) in levels {
        if let Some(hits) = sum.level_with_hits.get(*level) {
            let mut hits_vec: Vec<(&String, &usize)> = hits.iter().collect();
            hits_vec.sort_by(|a, b| b.1.cmp(a.1));
            let top_hits: Vec<(&String, &usize)> = hits_vec.into_iter().take(5).collect();
            let mut msgs: Vec<String> = top_hits
                .into_iter()
                .map(|(rule, count)| {
                    format!("{} ({})", rule, count.to_formatted_string(&Locale::en))
                })
                .collect();
            while msgs.len() < 5 {
                msgs.push("n/a".to_string());
            }
            table_data.push((*level, (color.rdg(false), msgs)));
        } else {
            let data = vec!["n/a".to_string(); 5];
            table_data.push((*level, (color.rdg(false), data)));
        }
    }
    let mut tb = Table::new();
    tb.load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_style(TableComponent::VerticalLines, ' ');
    let hlch = tb.style(TableComponent::HorizontalLines).unwrap();
    let tbch = tb.style(TableComponent::TopBorder).unwrap();
    for chunk in table_data.chunks(2) {
        let heads = chunk
            .iter()
            .map(|(level, (color, _))| Cell::new(format!("Top {} alerts:", level)).fg(rgb(color)))
            .collect::<Vec<_>>();
        let columns = chunk
            .iter()
            .map(|(_, (color, msgs))| {
                let msg = msgs
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                Cell::new(msg).fg(rgb(color))
            })
            .collect::<Vec<_>>();
        tb.add_row(heads)
            .set_style(TableComponent::MiddleIntersections, hlch)
            .set_style(TableComponent::TopBorderIntersections, tbch)
            .set_style(TableComponent::BottomBorderIntersections, hlch);
        tb.add_row(columns);
    }
    println!("{tb}");
    println!();
}

fn rgb(color: &Option<Color>) -> comfy_table::Color {
    match color {
        Some(Color::Rgb(255, 0, 0)) => comfy_table::Color::Rgb { r: 255, g: 0, b: 0 },
        Some(Color::Rgb(255, 175, 0)) => comfy_table::Color::Rgb {
            r: 255,
            g: 175,
            b: 0,
        },
        Some(Color::Rgb(255, 255, 0)) => comfy_table::Color::Rgb {
            r: 255,
            g: 255,
            b: 0,
        },
        Some(Color::Rgb(0, 255, 0)) => comfy_table::Color::Rgb { r: 0, g: 255, b: 0 },
        _ => comfy_table::Color::Rgb {
            r: 255,
            g: 255,
            b: 255,
        },
    }
}

fn print_detected_rule_authors(
    rule_author_counter: &HashMap<String, i128>,
    table_column_num: usize,
    no_color: bool,
) {
    let mut sorted_authors: Vec<(&String, &i128)> = rule_author_counter.iter().collect();
    sorted_authors.sort_by(|a, b| (-a.1).cmp(&(-b.1)));
    let authors_num = sorted_authors.len();
    let div = if authors_num <= table_column_num {
        1
    } else if authors_num % 4 != 0 {
        authors_num / table_column_num + 1
    } else {
        authors_num / table_column_num
    };
    let mut tb = Table::new();
    tb.load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_style(TableComponent::VerticalLines, ' ');
    let mut stored_by_column = vec![];
    let hlch = tb.style(TableComponent::HorizontalLines).unwrap();
    let tbch = tb.style(TableComponent::TopBorder).unwrap();
    for x in 0..table_column_num {
        let mut tmp = Vec::new();
        for y in 0..div {
            if y * table_column_num + x < sorted_authors.len() {
                // Limit length to 27 to prevent the table from wrapping
                let filter_author = if sorted_authors[y * table_column_num + x].0.len() <= 27 {
                    sorted_authors[y * table_column_num + x].0.to_string()
                } else {
                    format!("{}...", &sorted_authors[y * table_column_num + x].0[0..24])
                };
                tmp.push(format!(
                    "{} ({})",
                    filter_author,
                    sorted_authors[y * table_column_num + x].1
                ));
            }
        }
        if !tmp.is_empty() {
            stored_by_column.push(tmp);
        }
    }
    let mut output = vec![];
    for col_data in stored_by_column {
        output.push(col_data.join("\n"));
    }
    if !output.is_empty() {
        tb.add_row(output)
            .set_style(TableComponent::MiddleIntersections, hlch)
            .set_style(TableComponent::TopBorderIntersections, tbch)
            .set_style(TableComponent::BottomBorderIntersections, hlch);
    }
    p(Green.rdg(no_color), "Rule Authors:", true);
    p(None, &format!("{tb}"), true);
    println!();
}

fn print_timeline_hist(timestamps: &[i64], length: usize, side_margin_size: usize) {
    if timestamps.is_empty() {
        return;
    }
    if timestamps.len() < 5 {
        let msg = "Detection Frequency Timeline could not be displayed as there needs to be more than 5 events.";
        p(Some(Color::Rgb(255, 0, 0)), msg, false);
        p(None, "\n", true);
        return;
    }

    let title = "Detection Frequency Timeline";
    let header_row_space = (length - title.len()) / 2;
    let buf_wtr = BufferWriter::stdout(ColorChoice::Always);
    let mut wtr = buf_wtr.buffer();
    wtr.set_color(ColorSpec::new().set_fg(None)).ok();
    writeln!(wtr, "{}{}", " ".repeat(header_row_space), title).ok();
    println!();

    let timestamp_marker_max = if timestamps.len() < 2 {
        0
    } else {
        timestamps.len() - 2
    };
    let marker_num = min(timestamp_marker_max, 18);

    let (header_raw, footer_raw) =
        build_time_markers(timestamps, marker_num, length - (side_margin_size * 2));
    let sparkline = build_sparkline(timestamps, length - (side_margin_size * 2), 5_usize);
    for header_str in header_raw.lines() {
        writeln!(wtr, "{}{}", " ".repeat(side_margin_size - 1), header_str).ok();
    }
    for line in sparkline.lines() {
        writeln!(wtr, "{}{}", " ".repeat(side_margin_size - 1), line).ok();
    }
    for footer_str in footer_raw.lines() {
        writeln!(wtr, "{}{}", " ".repeat(side_margin_size - 1), footer_str).ok();
    }
    buf_wtr.print(&wtr).ok();
    println!();
}

fn load_profile(file_path: &str, geo_search: &Option<GeoIPSearch>) -> Vec<(String, String)> {
    let file = File::open(file_path).expect("Unable to open profile file");
    let reader = BufReader::new(file);
    let mut profile = vec![];

    for line in reader.lines() {
        let line = line.expect("Unable to read line");
        let parts: Vec<&str> = line.splitn(2, ':').collect();
        if parts.len() == 2 {
            let key = parts[0].trim();
            let val = parts[1].trim().trim_matches('\'');
            profile.push((key.to_string(), val.to_string()));
            if key == "SrcIP" && geo_search.is_some() {
                profile.push(("SrcASN".to_string(), "SrcASN".to_string()));
                profile.push(("SrcCity".to_string(), "SrcCity".to_string()));
                profile.push(("SrcCountry".to_string(), "SrcCountry".to_string()));
            }
        }
    }
    profile
}

fn get_value_from_event(
    key: &str,
    event: &Event,
    rule: &Rule,
    geo_ip: &mut Option<GeoIPSearch>,
) -> String {
    if let Some(geo) = geo_ip {
        if let Some(ip) = event.get("sourceIPAddress") {
            let ip = ip.value_to_string();
            if let Some(ip) = geo.convert(ip.as_str()) {
                if key == "SrcASN" {
                    return geo.get_asn(ip);
                } else if key == "SrcCity" {
                    return geo.get_city(ip);
                } else if key == "SrcCountry" {
                    return geo.get_country(ip);
                }
            } else {
                return ip;
            }
        }
    }
    if key.starts_with(".") {
        let key = key.strip_prefix(".").unwrap();
        if let Some(value) = event.get(key) {
            if key == "eventTime" {
                value.value_to_string().replace("T", " ").replace("Z", "")
            } else {
                value.value_to_string()
            }
        } else {
            "-".to_string()
        }
    } else if key.starts_with("sigma.") {
        let key = key.replace("sigma.", "");
        if key == "title" {
            rule.title.to_string()
        } else if key == "id" && rule.id.is_some() {
            rule.id.as_ref().unwrap().to_string()
        } else if key == "status" && rule.status.is_some() {
            format!("{:?}", rule.status.as_ref().unwrap()).to_lowercase()
        } else if key == "author" && rule.author.is_some() {
            rule.author.as_ref().unwrap().to_string()
        } else if key == "description" && rule.description.is_some() {
            rule.description.as_ref().unwrap().to_string()
        } else if key == "references" && rule.references.is_some() {
            format!("{:?}", rule.references.as_ref().unwrap())
        } else if key == "date" && rule.date.is_some() {
            rule.date.as_ref().unwrap().to_string()
        } else if key == "modified" && rule.modified.is_some() {
            rule.modified.as_ref().unwrap().to_string()
        } else if key == "tags" && rule.tags.is_some() {
            format!("{:?}", rule.tags.as_ref().unwrap())
        } else if key == "falsepositives" && rule.falsepositives.is_some() {
            format!("{:?}", rule.falsepositives.as_ref().unwrap())
        } else if key == "level" {
            format!("{:?}", rule.level.as_ref().unwrap()).to_lowercase()
        } else {
            "-".to_string()
        }
    } else {
        "-".to_string()
    }
}
