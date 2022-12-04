use serde_json::Result;
use std::{collections::HashMap, env, fs, path::PathBuf};
use swayipc::{Connection, Workspace};

use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct WorkspaceCli {
    /// The string "number".
    pub number: String,

    /// The workspace to switch to.
    pub workspace: i32,

    /// If the workspace does not exist yet, open it on this output.
    pub output: Option<String>,
}

/// Initializes the cli interface, connects to the sway ipc, returns the
/// provided (sanitized) output (for the given workspace) and whether the
/// provided workspace already exists.
pub fn initialize_workspace() -> (WorkspaceCli, Connection, Option<String>, bool) {
    let cli = WorkspaceCli::parse();

    let mut sway = Connection::new().expect("Cannot connect to sway via IPC.");

    let output = cli
        .output
        .as_ref()
        // If we are given an output then we sanitize it.
        .and_then(|output| output_if_exists(output.to_string(), &mut sway))
        // If we are not given an output or the sanitization threw it away we get the output for the provided workspace.
        .or_else(|| get_output_for_workspace(cli.workspace));

    // We check whether the provided workspace exists.
    let workspace_exists = workspace_exists(cli.workspace, &mut sway);

    (cli, sway, output, workspace_exists)
}

/// Returns the provided (optional) output if it is indeed connected.
///
/// Goes through the list of outputs and checks whether the provided output exists, i.e.,
/// checks whether the provided output is either the name (like `VGA-1`, `HDMI-A-3`, …) or a
/// combination of make, model, and serial number. If so the name is returned.
pub fn output_if_exists(output: String, sway: &mut Connection) -> Option<String> {
    for sway_output in sway.get_outputs().unwrap_or_default() {
        if output == sway_output.name {
            return Some(output);
        }
        if output
            == format!(
                "{} {} {}",
                sway_output.make, sway_output.model, sway_output.serial
            )
        {
            return Some(sway_output.name);
        }
    }
    None
}

pub fn get_config_path() -> PathBuf {
    [
        env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string()),
        "sway-workspaces-outputs.json".to_string(),
    ]
    .iter()
    .collect()
}

pub fn load_config() -> Result<HashMap<String, Vec<i32>>> {
    let config_path = get_config_path();
    let json = fs::read_to_string(config_path).unwrap_or_default();
    serde_json::from_str(&json)
}

pub fn save_config(config: &HashMap<String, Vec<i32>>) -> bool {
    let config_path = get_config_path();
    if let Ok(json) = serde_json::to_string(&config) {
        return fs::write(config_path, json).is_ok();
    }
    false
}

pub fn make_config(mappings: Vec<String>, sway: &mut Connection) -> HashMap<String, Vec<i32>> {
    let mut workspaces = HashMap::new();

    mappings
        .iter()
        .flat_map(|mapping| add_mapping(mapping, &mut workspaces, sway))
        .for_each(drop);

    workspaces
}

fn add_mapping(
    mapping: &str,
    workspaces: &mut HashMap<String, Vec<i32>>,
    sway: &mut Connection,
) -> Option<()> {
    let (output_str, workspace_str) = mapping.split_at(mapping.rfind(':')?);
    let output = output_if_exists(output_str.to_owned(), sway)?;
    if let Some(index) = workspace_str[1..].find('-') {
        let (left_str, right_str) = workspace_str[1..].split_at(index);
        let left = left_str.parse().ok()?;
        let right = right_str[1..].parse().ok()?;
        workspaces.insert(
            output,
            (if left <= right {
                left..=right
            } else {
                right..=left
            })
            .collect(),
        );
    } else {
        let number = workspace_str[1..].parse().ok()?;
        workspaces.insert(output, vec![number]);
    };
    Some(())
}

pub fn workspace_exists(workspace_num: i32, sway: &mut Connection) -> bool {
    sway.get_workspaces()
        .unwrap_or_default()
        .iter()
        .any(|workspace| workspace.num == workspace_num)
}

pub fn get_output_for_workspace(workspace_num: i32) -> Option<String> {
    let config = load_config().ok()?;

    for (output, workspaces) in config.into_iter() {
        if workspaces.contains(&workspace_num) {
            return Some(output);
        }
    }

    None
}

pub fn get_focused_workspace(sway: &mut Connection) -> Option<Workspace> {
    sway.get_workspaces()
        .unwrap_or_default()
        .into_iter()
        .find(|workspace| workspace.focused)
}

pub fn get_visible_workspace_for_output(
    output: &String,
    sway: &mut Connection,
) -> Option<Workspace> {
    sway.get_workspaces()
        .unwrap_or_default()
        .into_iter()
        .find(|workspace| &workspace.output == output && workspace.visible)
}
