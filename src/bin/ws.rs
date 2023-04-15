#![feature(is_some_and)]

use clap::{builder::TypedValueParser, Parser};
use itertools::Itertools;
use std::{collections::HashMap, fs};
use swayipc::{Event, EventType, Workspace};
use thiserror::Error as ThisError;

#[derive(clap::Parser, Debug)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    /// The file for the output-to-workspace mapping.
    #[arg(short, long, value_hint = clap::ValueHint::FilePath, default_value = "$XDG_RUNTIME_DIR/ws.json")]
    mapping_file: String,
    /// The file where the last active workspace is stored.
    #[arg(short, long, value_hint = clap::ValueHint::FilePath, default_value = "$XDG_RUNTIME_DIR/ws-prev.json")]
    previous_file: String,
    /// Only show commands instead of executing them.
    #[arg(short = 'n', long)]
    dry_run: bool,
}

#[derive(clap::Subcommand, Debug)]
enum Commands {
    /// Focus a given workspace
    Focus(Focus),
    /// Move the focused container to the specified workspace
    Move(Move),
    /// Set the output-to-workspace mapping
    Map(Map),
    /// Run in background to monitor workspace changes
    Monitor,
}

#[derive(clap::Args, Debug)]
#[command(group(clap::ArgGroup::new("workspace").args(["number", "name"]).multiple(true).required(true)))]
struct Focus {
    #[arg(long)]
    no_auto_back_and_forth: bool,
    #[arg(long)]
    number: Option<i32>,
    name: Option<String>,
}

#[derive(clap::Args, Debug)]
#[command(group(clap::ArgGroup::new("workspace").args(["number", "name"]).required(true)))]
struct Move {
    #[arg(long)]
    no_auto_back_and_forth: bool,
    #[arg(long)]
    number: Option<i32>,
    name: Option<String>,
}

#[derive(clap::Args, Debug)]
struct Map {
    /// Maps (multiple) workspace(s) to one output in the forms
    /// `output:num` or `output:from-to` or `output:num1,num2,num3,...`.
    /// Setting an output a second time removes previous settings.
    #[arg(required = true, value_name = "OUTPUT:WORKSPACE(S)", value_parser = clap::builder::StringValueParser::new().try_map(map_validator))]
    maps: Vec<(String, Vec<i32>)>,
}

fn map_validator(string: String) -> Result<(String, Vec<i32>), String> {
    let (output, workspace_str) = string
        .split_once(':')
        .ok_or("must contain colon as separator")?;
    let mut workspaces = Vec::new();
    for part in workspace_str.split(',') {
        if part.contains('-') {
            let (left, right) = part
                .split_once('-')
                .ok_or("cannot split on '-' after ensuring '-' is in string")?;
            let left: i32 = left.parse().map_err(|err| format!("'{left}' - {err}"))?;
            let right: i32 = right.parse().map_err(|err| format!("'{right}' - {err}"))?;
            let (left, right) = if left <= right {
                (left, right)
            } else {
                (right, left)
            };
            for num in left..=right {
                workspaces.push(num);
            }
        } else {
            let num = part.parse().map_err(|err| format!("'{part}' - {err}"))?;
            workspaces.push(num);
        }
    }
    workspaces.sort();
    workspaces.dedup();
    Ok((output.to_owned(), workspaces))
}

fn main() {
    let mut cli = Cli::parse();

    if cli.mapping_file.starts_with("$XDG_RUNTIME_DIR") {
        cli.mapping_file = cli.mapping_file.replace(
            "$XDG_RUNTIME_DIR",
            &std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned()),
        )
    }
    if cli.previous_file.starts_with("$XDG_RUNTIME_DIR") {
        cli.previous_file = cli.previous_file.replace(
            "$XDG_RUNTIME_DIR",
            &std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned()),
        )
    }

    let sway = Sway::new(&cli.mapping_file, &cli.previous_file, cli.dry_run)
        .expect("Cannot connect to sway ipc.");

    match cli.command {
        Commands::Focus(args) => ws_focus(sway, args),
        Commands::Move(args) => ws_move(sway, args),
        Commands::Map(args) => ws_map(sway, args),
        Commands::Monitor => ws_monitor(sway),
    }
    .unwrap()
}

fn ws_focus(mut sway: Sway, args: Focus) -> Fallible<()> {
    sway.update_workspaces()?;

    let target = sway.workspace_by_num_or_name(args.number, args.name.as_deref());
    let focused = sway.focused_workspace().ok_or(Error::NoFocusedWorkspace)?;

    // If the target workspace already exists
    if let Some(target) = target {
        // If the target is already focused and --no-auto-back-and-forth was passed, abort here
        if target.num == focused.num && target.name == focused.name && args.no_auto_back_and_forth {
            return Ok(());
        }
        // Just focus the target workspace. This will either focus it (if not focused yet) or go to previously
        // focused workspace if auto-back-and-forth is enabled.
        sway.connection
            .workspace(args.number, args.name.as_deref())?;
        return Ok(());
    }

    // If workspace is not given by a number, just select the workspace
    if args.number.is_none() {
        sway.connection.workspace_name(&args.name.unwrap())?;
        return Ok(());
    }

    let number = args.number.unwrap();

    // Store name of the previously focused workspace as the following calls seem (to the compiler) to invalidate the data
    let focused_name = focused.name.to_owned();

    // Find out on which output the numbered workspace should be shown
    sway.load_mapping()?;
    sway.update_outputs()?;
    if let Some((output_str, _)) = sway.mapping.iter().find(|(_, ws)| ws.contains(&number)) {
        let focused_output = sway.focused_output().ok_or(Error::NoFocusedOutput)?;
        if &focused_output.name == output_str {
            // We are on the correct output already, just select workspace
            sway.connection
                .workspace(args.number, args.name.as_deref())?;
            return Ok(());
        }
        // 1. focus the desired output
        sway.connection.focus_output(output_str)?;
        // 2. select the desired workspace
        sway.connection
            .workspace(args.number, args.name.as_deref())?;
        // 3. select the initially focused workspace
        sway.connection.workspace_name(&focused_name)?;
        // 4. select the desired workspace
        sway.connection
            .workspace(args.number, args.name.as_deref())?;
    } else {
        // We could not find the desired output, just select it
        sway.connection
            .workspace(args.number, args.name.as_deref())?;
        return Ok(());
    }

    Ok(())
}

const WS_MOVE_MARKER: &str = "__ws_move__";

fn ws_move(mut sway: Sway, args: Move) -> Fallible<()> {
    sway.connection
        .move_to_workspace(args.number, args.name.as_deref())?;
    return Ok(());

    sway.update_workspaces()?;
    // let target = sway.workspace_by_num_or_name(args.number, args.name.as_deref());
    // let focused = sway.focused_workspace().ok_or(Error::NoFocusedWorkspace)?;
    // let focused_num = focused.num;
    // let focused_name = focused.name.to_owned();

    // Plan
    // 0. Abort if no-back-and-forth is provided and focused workspace is selected one
    // 1. Mark selected window
    // 2. Move window to target workspace (this may create the workspace)
    // 3. Find target workspace via mark
    // 4. If target workspace does not have other windows and it is on the wrong output
    // 4.1. Get focused workspace on output
    // 4.2. Move workspace to output
    // 4.3. Focus previously focused workspace

    // 0. Abort if no-back-and-forth is provided and focused workspace is selected one
    if args.no_auto_back_and_forth {
        let target = sway.workspace_by_num_or_name(args.number, args.name.as_deref());
        let focused = sway.focused_workspace().ok_or(Error::NoFocusedWorkspace)?;
        if target.is_some_and(|t| t.num == focused.num && t.name == focused.name) {
            return Ok(());
        }
    }

    // 1. Mark selected window
    sway.connection.mark_add(WS_MOVE_MARKER)?;

    // 2. Move window to target workspace (this may create the workspace)
    sway.connection
        .move_to_workspace(args.number, args.name.as_deref())?;

    // 3. Find target workspace via mark
    let (ws_num, ws_name, ws_windows, output_name) =
        sway.connection.get_workspace_with_mark(WS_MOVE_MARKER)?;
    // It has other windows then the moved one or the workspace has no number - we are done
    if ws_windows > 1 || ws_num < 0 {
        return Ok(());
    }

    // 4. If target workspace does not have other windows and it is on the wrong output
    // Need to load mapping first
    sway.load_mapping()?;
    // Find the output which should contain the target workspace but does not
    let found = sway
        .mapping
        .iter()
        .find(|(o, w)| w.contains(&&ws_num) && o != &&output_name);
    if found.is_none() {
        return Ok(());
    }
    let (output, _) = found.unwrap();
    // 4.1. Get focused workspace on output

    // // Obtain the (new) target. This may happen when selecting the focused workspace as target and auto-back-and-forth is enabled.
    // let target = if let Some(target) = target {
    //     // If the target is already focused …
    //     if target.num == focused_num && target.name == focused_name {
    //         // … and --no-auto-back-and-forth was passed, abort here
    //         if args.no_auto_back_and_forth {
    //             return Ok(());
    //         }
    //         // otherwise we need to find out which one is the auto-back-and-forth workspace
    //         sway.connection
    //             .cmd_workspace(args.number, args.name.as_deref())?;
    //         sway.update_workspaces()?;
    //         let newly_focused = sway.focused_workspace().ok_or(Error::NoFocusedWorkspace)?;
    //         // We still focus the same workspace, no movement necessary
    //         if newly_focused.num == focused_num && newly_focused.name == focused_name {
    //             return Ok(());
    //         }
    //         // We now now which workspace we need to move to.
    //         // 1. Move back to the originally focused workspace
    //         sway.connection.cmd_workspace(
    //             if focused_num > -1 {
    //                 Some(focused_num)
    //             } else {
    //                 None
    //             },
    //             if !focused_name.is_empty() {
    //                 Some(&focused_name)
    //             } else {
    //                 None
    //             },
    //         )?;
    //         // 2. Return the newly focused workspace as target
    //         Some(newly_focused)
    //     } else {
    //         Some(target)
    //     }
    // } else {
    //     None
    // };

    // // If the target workspace already exists
    // if let Some(target) = target {
    //     // If the target is already focused …
    //     if target.num == focused_num && target.name == focused_name {
    //         // … and --no-auto-back-and-forth was passed, abort here
    //         if args.no_auto_back_and_forth {
    //             return Ok(());
    //         }
    //         // otherwise we need to find out which one is the auto-back-and-forth workspace
    //         sway.connection
    //             .cmd_workspace(args.number, args.name.as_deref())?;
    //         sway.update_workspaces()?;
    //         let newly_focused = sway.focused_workspace().ok_or(Error::NoFocusedWorkspace)?;
    //         // We still focus the same workspace, no movement necessary
    //         if newly_focused.num == focused_num && newly_focused.name == focused_name {
    //             return Ok(());
    //         }
    //         // We now now which workspace we need to move to
    //         // 1. Focus back on the original workspace
    //         // sway.connection.cmd_workspace(num, name);
    //     }
    //     // Just focus the target workspace. This will either focus it (if not focused yet) or go to previously
    //     // focused workspace if auto-back-and-forth is enabled.
    //     sway.connection
    //         .cmd_workspace(args.number, args.name.as_deref())?;
    //     return Ok(());
    // }

    // // If workspace is not given by a number, just select the output
    // if args.number.is_none() {
    //     sway.connection.cmd_workspace_name(&args.name.unwrap())?;
    //     return Ok(());
    // }

    // let number = args.number.unwrap();

    // // Store name of the previously focused workspace as the following calls seem (to the compiler) to invalidate the data
    // let focused_name = focused.name.to_owned();

    // // Find out on which output the numbered workspace should be shown
    // sway.load_mapping()?;
    // sway.update_outputs()?;
    // if let Some((output_str, _)) = sway.mapping.iter().find(|(_, ws)| ws.contains(&number)) {
    //     let focused_output = sway.focused_output().ok_or(Error::NoFocusedOutput)?;
    //     if &focused_output.name == output_str {
    //         // We are on the correct output already, just select workspace
    //         sway.connection
    //             .cmd_workspace(args.number, args.name.as_deref())?;
    //         return Ok(());
    //     }
    //     // 1. focus the desired output
    //     sway.connection.cmd_output(output_str)?;
    //     // 2. select the desired workspace
    //     sway.connection
    //         .cmd_workspace(args.number, args.name.as_deref())?;
    //     // 3. select the initially focused workspace
    //     sway.connection.cmd_workspace_name(&focused_name)?;
    //     // 4. select the desired workspace
    //     sway.connection
    //         .cmd_workspace(args.number, args.name.as_deref())?;
    // } else {
    //     // We could not find the desired output, just select it
    //     sway.connection
    //         .cmd_workspace(args.number, args.name.as_deref())?;
    //     return Ok(());
    // }

    Ok(())
}

fn ws_map(mut sway: Sway, args: Map) -> Fallible<()> {
    sway.update_outputs()?;
    for (output_str, workspaces) in args.maps.into_iter() {
        if let Some(outputs) = sway.outputs() {
            if let Some(output) = outputs.iter().find(|o| {
                o.name == output_str || output_str == format!("{} {} {}", o.make, o.model, o.serial)
            }) {
                sway.mapping.insert(output.name.to_owned(), workspaces);
            }
        }
    }
    // sway.connection.run("reload")?;
    // for (output, workspaces) in sway.mapping.iter() {
    //     for workspace in workspaces.iter() {
    //         sway.connection
    //             .run(format!("workspace {} output {}", workspace, output))?;
    //     }
    // }
    sway.save_mapping()?;

    Ok(())
}

fn ws_monitor(mut sway: Sway) -> ! {
    // Subscribe to all workspace events
    let event_types = [EventType::Workspace];
    let mut events = sway
        .connection
        .sway
        .subscribe(event_types)
        .expect("Cannot subscribe to sway events.");

    loop {
        let event = events.next();
        if let Some(Ok(Event::Workspace(ev))) = event {
            if let Some(old) = ev.old {
                if let Some(num) = old.num {
                    if let Some(name) = old.name {
                        if let Ok(data) = serde_json::to_string(&(name, num)) {
                            if let Ok(_) = fs::write(sway.previous_file, data) {}
                        }
                    }
                }
            }
        }
    }
}

type Fallible<T> = Result<T, Error>;

#[derive(Debug, ThisError)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    SerdeJson(#[from] serde_json::Error),
    #[error(transparent)]
    Sway(#[from] swayipc::Error),
    #[error("no focused workspace exists")]
    NoFocusedWorkspace,
    #[error("could not get workspaces")]
    NoWorkspaces,
    #[error("no focused output exists")]
    NoFocusedOutput,
    #[error("could not get outputs")]
    NoOutputs,
    #[error("you must provide either num or name")]
    NeitherNumNorNameProvided,
    #[error("previously set mark was not found")]
    MarkNotFound,
    #[error("tree does not return expected output")]
    UnexpectedTree,
}

struct Sway<'a> {
    connection: Connection,
    workspaces: Option<Vec<swayipc::Workspace>>,
    outputs: Option<Vec<swayipc::Output>>,
    mapping_file: &'a str,
    previous_file: &'a str,
    mapping: HashMap<String, Vec<i32>>,
}

struct Connection {
    sway: swayipc::Connection,
    dry_run: bool,
}

impl Connection {
    fn run_command<T: AsRef<str> + std::fmt::Display>(
        &mut self,
        payload: T,
    ) -> Fallible<Vec<swayipc::Fallible<()>>> {
        if self.dry_run {
            println!("SWAY: \u{1b}[1;34m{payload}\u{1b}[0m");
            Ok(Vec::new())
        } else {
            self.sway.run_command(payload).map_err(Error::Sway)
        }
    }

    fn run<T: AsRef<str> + std::fmt::Display>(&mut self, payload: T) -> Fallible<()> {
        self.run_command(payload)?;
        Ok(())
    }

    pub fn workspace(&mut self, num: Option<i32>, name: Option<&str>) -> Fallible<()> {
        if let Some(num) = num {
            if let Some(name) = name {
                let payload = format!("workspace number {num}:{name}");
                self.run(payload)
            } else {
                self.workspace_num(num)
            }
        } else if let Some(name) = name {
            self.workspace_name(name)
        } else {
            Err(Error::NeitherNumNorNameProvided)
        }
    }

    pub fn workspace_num(&mut self, num: i32) -> Fallible<()> {
        self.run(format!("workspace number {num}"))
    }

    pub fn workspace_name(&mut self, name: &str) -> Fallible<()> {
        self.run(format!("workspace {name}"))
    }

    pub fn move_to_workspace(&mut self, num: Option<i32>, name: Option<&str>) -> Fallible<()> {
        if let Some(num) = num {
            if let Some(name) = name {
                self.run(format!("move to workspace number {num}:{name}"))
            } else {
                self.move_to_workspace_num(num)
            }
        } else if let Some(name) = name {
            self.move_to_workspace_name(name)
        } else {
            Err(Error::NeitherNumNorNameProvided)
        }
    }

    pub fn move_to_workspace_num(&mut self, num: i32) -> Fallible<()> {
        self.run(format!("move to workspace number {num}"))
    }

    pub fn move_to_workspace_name(&mut self, name: &str) -> Fallible<()> {
        self.run(format!("move to workspace {name}"))
    }

    pub fn move_workspace_to_output(&mut self, output: &str) -> Fallible<()> {
        self.run(format!("move workspace to output {output}"))
    }

    pub fn focus_output(&mut self, name: &str) -> Fallible<()> {
        self.run(format!("focus output {name}"))
    }

    pub fn mark_add(&mut self, mark: &str) -> Fallible<()> {
        self.run(format!("mark --add {mark}"))
    }

    pub fn mark_remove(&mut self, mark: &str) -> Fallible<()> {
        self.run(format!("unmark {mark}"))
    }

    pub fn mark_remove_all(&mut self) -> Fallible<()> {
        self.run(format!("unmark"))
    }

    pub fn get_workspace_with_mark(
        &mut self,
        mark: &str,
    ) -> Fallible<(i32, String, usize, String)> {
        let tree = self.sway.get_tree()?;
        for output in tree.nodes {
            for workspace in output.nodes {
                for window in workspace.nodes.iter() {
                    if window.marks.iter().any(|m| m == mark) {
                        let ws_num = workspace.num.ok_or(Error::UnexpectedTree)?;
                        let ws_name = workspace.name.ok_or(Error::UnexpectedTree)?;
                        let output_name = output.name.ok_or(Error::UnexpectedTree)?;
                        return Ok((ws_num, ws_name, workspace.nodes.len(), output_name));
                    }
                }
            }
        }
        Err(Error::MarkNotFound)
    }
}

impl Sway<'_> {
    pub fn new<'a>(
        mapping_file: &'a str,
        previous_file: &'a str,
        dry_run: bool,
    ) -> Fallible<Sway<'a>> {
        Ok(Sway {
            connection: Connection {
                sway: swayipc::Connection::new()?,
                dry_run,
            },
            workspaces: None,
            outputs: None,
            mapping_file,
            previous_file,
            mapping: HashMap::new(),
        })
    }

    pub fn get_previous_workspace(&mut self) -> Fallible<(String, i32)> {
        let json = fs::read_to_string(self.previous_file)?;
        let result = serde_json::from_str(&json)?;
        Ok(result)
    }

    pub fn save_focused_workspace(&mut self, num: i32, name: &str) -> Fallible<()> {
        Ok(())
    }

    pub fn load_mapping(&mut self) -> Fallible<()> {
        let json = fs::read_to_string(self.mapping_file)?;
        self.mapping = serde_json::from_str(&json)?;
        Ok(())
    }

    pub fn save_mapping(&mut self) -> Fallible<()> {
        let data = serde_json::to_string(&self.mapping)?;
        fs::write(self.mapping_file, data)?;
        Ok(())
    }

    pub fn workspace_by_num(&self, num: i32) -> Option<&swayipc::Workspace> {
        self.workspaces()
            .and_then(|wss| wss.iter().find(|ws| ws.num == num))
    }

    pub fn workspace_by_name(&self, name: &str) -> Option<&swayipc::Workspace> {
        self.workspaces()
            .and_then(|wss| wss.iter().find(|ws| ws.name == name))
    }

    pub fn workspace_by_num_or_name(
        &self,
        num: Option<i32>,
        name: Option<&str>,
    ) -> Option<&swayipc::Workspace> {
        self.workspaces().and_then(|wss| {
            wss.iter().find(|ws| {
                name.is_some_and(|name| name == ws.name) || num.is_some_and(|num| num == ws.num)
            })
        })
    }

    pub fn workspaces(&self) -> Option<&Vec<swayipc::Workspace>> {
        self.workspaces.as_ref()
    }

    pub fn focused_workspace(&self) -> Option<&swayipc::Workspace> {
        self.workspaces()?.iter().find(|ws| ws.focused)
    }

    fn update_workspaces(&mut self) -> Fallible<()> {
        if self.workspaces.is_none() {
            self.workspaces = Some(self.connection.sway.get_workspaces()?);
        }
        Ok(())
    }

    pub fn reset_workspaces(&mut self) {
        self.workspaces = None;
    }

    fn force_update_workspaces(&mut self) -> Fallible<()> {
        self.reset_workspaces();
        self.update_workspaces()
    }

    // ########################################################################

    pub fn output_by_name_or_identifier(
        &self,
        name: Option<&str>,
        identifier: Option<&str>,
    ) -> Option<&swayipc::Output> {
        self.outputs().and_then(|os| {
            os.iter().find(|o| {
                name.is_some_and(|name| name == o.name)
                    || identifier.is_some_and(|identifier| {
                        identifier == format!("{} {} {}", o.make, o.model, o.serial)
                    })
            })
        })
    }

    pub fn outputs(&self) -> Option<&Vec<swayipc::Output>> {
        self.outputs.as_ref()
    }

    pub fn focused_output(&self) -> Option<&swayipc::Output> {
        self.outputs()?.iter().find(|ws| ws.focused)
    }

    fn update_outputs(&mut self) -> Fallible<()> {
        if self.outputs.is_none() {
            self.outputs = Some(self.connection.sway.get_outputs()?);
        }
        Ok(())
    }

    pub fn reset_outputs(&mut self) {
        self.outputs = None;
    }

    fn force_update_outputs(&mut self) -> Fallible<()> {
        self.reset_outputs();
        self.update_outputs()
    }
}
