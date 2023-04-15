use clap::{ArgGroup, Parser, ValueHint};
use itertools::Itertools;
use serde::Serialize;
use serde_json::Value;
use std::fmt::Write;
use std::{
    collections::HashMap,
    ffi::CStr,
    fs::File,
    io::{BufRead, BufReader},
    os::raw::c_char,
    process::exit,
};
use swayipc::{Connection, Event, EventType, Input};
use tinytemplate::{error::Error, TinyTemplate};
use xkbregistry::{
    rxkb_context_new, rxkb_context_parse_default_ruleset, rxkb_context_unref, rxkb_layout_first,
    rxkb_layout_get_brief, rxkb_layout_get_description, rxkb_layout_get_name,
    rxkb_layout_get_variant, rxkb_layout_next, RXKB_CONTEXT_LOAD_EXOTIC_RULES,
};

/// sway keyboard information reporting for status bars.
///
/// This tool prints
#[derive(Parser, Debug)]
#[command(author, version, about)]
#[clap(group(ArgGroup::new("in").args(["include", "include_file"]).multiple(true).conflicts_with("ex")))]
#[clap(group(ArgGroup::new("ex").args(["exclude", "exclude_file"]).multiple(true)))]
#[clap(group(ArgGroup::new("any").args(["include", "include_file", "exclude", "exclude_file"]).required(true).multiple(true)))]
struct Cli {
    /// Keyboard identifier (e.g., '1:1:AT_Translated_Set_2_keyboard') to be included
    #[arg(short, long)]
    include: Vec<String>,

    /// A config file containing the keyboard identifiers to be included
    #[arg(short = 'n', long, value_hint = ValueHint::FilePath)]
    include_file: Option<String>,

    /// Keyboard identifier (e.g., '1:1:AT_Translated_Set_2_keyboard') to be excluded
    #[arg(short, long)]
    exclude: Vec<String>,

    /// A config file containing the keyboard identifiers to be excluded
    #[arg(short = 'x', long, value_hint = ValueHint::FilePath)]
    exclude_file: Option<String>,

    /// The output string formatting
    #[arg(short, long, default_value = "{result}")]
    format: String,

    /// The output string formatting for a single keyboard
    #[arg(short = 's', long, default_value = "{flag}")]
    format_single: String,

    /// The output string separator for multiple keyboards
    #[arg(short = 'p', long, default_value = "")]
    format_separator: String,

    /// The tooltip string formatting for a single keyboard
    #[arg(short, long, default_value = "<b>Keyboards</b>\n{result}")]
    tooltip: String,

    /// The tooltip string formatting for a single keyboard
    #[arg(short = 'o', long, default_value = "{keyboard}: {description}")]
    tooltip_single: String,

    /// The tooltip string separator for multiple keyboards
    #[arg(short = 'r', long, default_value = "\n")]
    tooltip_separator: String,
}

static JSON_OUTPUT: &str = "\\{\"text\":\"{text}\",\"tooltip\":\"{tooltip}\"}";

pub fn format_json_escaped(value: &Value, output: &mut String) -> Result<(), Error> {
    match value {
        Value::Null => Ok(()),
        Value::Bool(b) => {
            write!(output, "{b}")?;
            Ok(())
        }
        Value::Number(n) => {
            write!(output, "{n}")?;
            Ok(())
        }
        Value::String(s) => {
            output.push_str(&s.replace('"', "\\\"").replace('\n', "\\n"));
            Ok(())
        }
        _ => Err(Error::GenericError {
            msg: "Expected a printable value but found array or object.".to_string(),
        }),
    }
}

fn main() {
    let cli = Cli::parse();
    let mut sway = Connection::new().expect("Cannot connect to sway ipc socket.");
    // Get a list of all interface identifiers that should be matched and whether the match should be inclusive or exclusive
    let (matches, include) = get_include_exclude(&cli);

    // Load all layouts for all keyboards present and matching
    let mut layouts = initialize_layouts(&matches, include, &mut sway);

    let mut templater = TinyTemplate::new();
    templater.set_default_formatter(&format_json_escaped);

    if let Err(err) = templater.add_template("json", JSON_OUTPUT) {
        println!("Builtin json template is invalid template: {err}");
        exit(1);
    }

    if let Err(err) = templater.add_template("format", &cli.format) {
        println!("`format` string is invalid template: {err}");
        exit(1);
    }
    if let Err(err) = templater.add_template("format_single", &cli.format_single) {
        println!("`format_single` string is invalid template: {err}");
        exit(1);
    }
    if let Err(err) = templater.add_template("tooltip", &cli.tooltip) {
        println!("`tooltip` string is invalid template: {err}");
        exit(1);
    }
    if let Err(err) = templater.add_template("tooltip_single", &cli.tooltip_single) {
        println!("`tooltip_single` string is invalid template: {err}");
        exit(1);
    }

    // Before entering the event loop, print out the keyboard situation
    output_keyboards(
        &layouts,
        &templater,
        &cli.format_separator,
        &cli.tooltip_separator,
    );

    // Subscribe to all input events
    let event_types = [EventType::Input];
    let mut events = sway
        .subscribe(event_types)
        .expect("Cannot subscribe to sway events.");

    loop {
        let event = events.next();
        // Only look at input events (other events should never appear here, anyway)
        if let Some(Ok(Event::Input(ev))) = event {
            // Ignore events that are not keyboard events or don't match our criteria
            if (ev.input.input_type != "keyboard")
                || (include && !matches.contains(&ev.input.identifier))
                || (!include && matches.contains(&ev.input.identifier))
            {
                continue;
            }
            match ev.change {
                // If a keyboard was removed, remove the corresponding entry from our mapping
                swayipc::InputChange::Removed => {
                    layouts.remove(&ev.input.identifier);
                }
                // If a keyboard was added or a layout changed, store the (new) layout in our mapping
                swayipc::InputChange::Added
                | swayipc::InputChange::XkbKeymap
                | swayipc::InputChange::XkbLayout => {
                    if let Some(layout) =
                        get_layout_for_name(&ev.input.xkb_active_layout_name.unwrap_or_default())
                    {
                        layouts.insert(ev.input.identifier, (ev.input.name, layout));
                    }
                }
                // Ignore all other events
                _ => (),
            };

            // Print out the (new) keyboard situation
            output_keyboards(
                &layouts,
                &templater,
                &cli.format_separator,
                &cli.tooltip_separator,
            );
        }
    }
}

/// Outputs a json representation of the current keyboard situation.
fn output_keyboards(
    layouts: &HashMap<String, (String, Layout)>,
    templater: &TinyTemplate,
    format_separator: &str,
    tooltip_separator: &str,
) {
    let single_contexts: Vec<SingleContext> = layouts
        .iter()
        .sorted_by_key(|x| x.0)
        .map(|x| SingleContext {
            keyboard: x.1 .0.to_owned(),
            description: x.1 .1.description.to_owned(),
            name: x.1 .1.name.to_owned(),
            variant: x.1 .1.variant.to_owned().unwrap_or_default(),
            brief: x.1 .1.brief.to_owned().unwrap_or_default(),
            flag: x.1 .1.flag(),
        })
        .collect();

    let format_singles = single_contexts
        .iter()
        .map(|c| templater.render("format_single", c))
        .filter_map(|s| s.ok())
        .join(format_separator);

    let tooltip_singles = single_contexts
        .iter()
        .map(|c| templater.render("tooltip_single", c))
        .filter_map(|s| s.ok())
        .join(tooltip_separator);

    let text = templater
        .render(
            "format",
            &ResultContext {
                result: format_singles,
            },
        )
        .unwrap();
    let tooltip = templater
        .render(
            "tooltip",
            &ResultContext {
                result: tooltip_singles,
            },
        )
        .unwrap();

    println!(
        "{}",
        templater
            .render("json", &GlobalContext { text, tooltip })
            .unwrap()
    );
}

/// Return a list of elements to be included/excluded and a flag telling us to include or exclude.
fn get_include_exclude(cli: &Cli) -> (Vec<String>, bool) {
    let include = !cli.include.is_empty() || cli.include_file.is_some();
    let list = if include {
        build_clude_list(&cli.include, &cli.include_file)
    } else {
        build_clude_list(&cli.exclude, &cli.exclude_file)
    };

    (list, include)
}

fn build_clude_list(list: &Vec<String>, opt_file_name: &Option<String>) -> Vec<String> {
    let mut result = list.to_owned();
    if let Some(file_name) = opt_file_name {
        if let Ok(file) = File::open(file_name) {
            for line in BufReader::new(file).lines().flatten() {
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                result.push(line);
            }
        }
    }

    result
}

/// Convert a given char pointer from a C function into an optional String.
///
/// Returns the converted string if the pointer is valid and the underlying memory can be interpreted as an utf8 string.
/// Returns `None` otherwise.
fn c_char_ptr_to_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        None
    } else {
        let c_str = unsafe { CStr::from_ptr(ptr) };
        c_str.to_str().map(|s| s.to_owned()).ok()
    }
}

#[derive(Debug)]
struct Layout {
    description: String,
    name: String,
    variant: Option<String>,
    brief: Option<String>,
}

impl Layout {
    fn flag(&self) -> String {
        if self.name.len() != 2 {
            return "".to_string();
        }
        let bytes = self.name.as_bytes();
        let data = vec![
            0xf0,
            0x9f,
            0x87,
            bytes[0] + 0x45,
            0xf0,
            0x9f,
            0x87,
            bytes[1] + 0x45,
        ];

        String::from_utf8(data).unwrap_or_default()
    }
}

fn initialize_layouts(
    matches: &[String],
    include: bool,
    sway: &mut Connection,
) -> HashMap<String, (String, Layout)> {
    let mut names = HashMap::new();

    for input in sway.get_inputs().unwrap_or_default() {
        if (input.input_type != "keyboard")
            || (include && !matches.contains(&input.identifier))
            || (!include && matches.contains(&input.identifier))
        {
            continue;
        }
        names.insert(input.identifier.to_owned(), input);
    }

    get_layouts_from_names(&names)
}

fn get_layouts_from_names(names: &HashMap<String, Input>) -> HashMap<String, (String, Layout)> {
    let mut layouts = HashMap::new();

    let ctx = unsafe { rxkb_context_new(RXKB_CONTEXT_LOAD_EXOTIC_RULES) };
    if ctx.is_null() {
        return layouts;
    }
    if !unsafe { rxkb_context_parse_default_ruleset(ctx) } {
        unsafe { rxkb_context_unref(ctx) };
        return layouts;
    }
    let mut layout = unsafe { rxkb_layout_first(ctx) };
    while !layout.is_null() && layouts.len() < names.len() {
        if let Some(description) =
            c_char_ptr_to_string(unsafe { rxkb_layout_get_description(layout) })
        {
            if let Some(name) = c_char_ptr_to_string(unsafe { rxkb_layout_get_name(layout) }) {
                for (identifier, input) in names.iter() {
                    if matches!(&input.xkb_active_layout_name, Some(d) if d == &description) {
                        layouts.insert(
                            identifier.to_owned(),
                            (
                                input.name.to_owned(),
                                Layout {
                                    description: description.to_owned(),
                                    name: name.to_owned(),
                                    variant: c_char_ptr_to_string(unsafe {
                                        rxkb_layout_get_variant(layout)
                                    }),
                                    brief: c_char_ptr_to_string(unsafe {
                                        rxkb_layout_get_brief(layout)
                                    }),
                                },
                            ),
                        );
                    }
                }
            }
        }

        layout = unsafe { rxkb_layout_next(layout) };
    }
    unsafe { rxkb_context_unref(ctx) };

    layouts
}

fn get_layout_for_name(layout_name: &String) -> Option<Layout> {
    let ctx = unsafe { rxkb_context_new(RXKB_CONTEXT_LOAD_EXOTIC_RULES) };
    if ctx.is_null() {
        return None;
    }
    if !unsafe { rxkb_context_parse_default_ruleset(ctx) } {
        unsafe { rxkb_context_unref(ctx) };
        return None;
    }
    let mut layout = unsafe { rxkb_layout_first(ctx) };
    while !layout.is_null() {
        if let Some(description) =
            c_char_ptr_to_string(unsafe { rxkb_layout_get_description(layout) })
        {
            if layout_name == &description {
                let name = c_char_ptr_to_string(unsafe { rxkb_layout_get_name(layout) })
                    .unwrap_or_default();
                let variant = c_char_ptr_to_string(unsafe { rxkb_layout_get_variant(layout) });
                let brief = c_char_ptr_to_string(unsafe { rxkb_layout_get_brief(layout) });
                unsafe { rxkb_context_unref(ctx) };
                return Some(Layout {
                    description,
                    name,
                    variant,
                    brief,
                });
            }
        }

        layout = unsafe { rxkb_layout_next(layout) };
    }
    unsafe { rxkb_context_unref(ctx) };
    None
}

#[derive(Serialize)]
struct SingleContext {
    keyboard: String,
    description: String,
    name: String,
    variant: String,
    brief: String,
    flag: String,
}

#[derive(Serialize)]
struct GlobalContext {
    text: String,
    tooltip: String,
}

#[derive(Serialize)]
struct ResultContext {
    result: String,
}
