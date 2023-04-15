use clap::Parser;
use std::collections::{HashMap, HashSet};
use swayipc::Connection;
use swaytools::{make_config, save_config};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct MappingCli {
    /// An output workspace mapping in the form "output:number" or "output:from-to", e.g., VGA-1:1-10 or "Dell X2353 0x2342:22"
    mapping: Vec<String>,
}

fn main() {
    let cli = MappingCli::parse();
    let mut sway = Connection::new().expect("Cannot connect to sway via IPC.");

    // Create a configuration mapping from the mapping strings on the command line.
    let config = make_config(cli.mapping, &mut sway);
    // Save the configuration to a file.
    save_config(&config);
    // Actually move the workspaces according to the configuration.
    move_workspaces(&config, &mut sway)
}

/// Move all workspaces in `mappings` to the correct outputs.
///
/// `mappings` is a mapping from output (e.g., `VGA-1`) to a list of workspaces
/// to be shown on this output.
fn move_workspaces(mappings: &HashMap<String, Vec<i32>>, sway: &mut Connection) {
    // Take a copy of all outputs to ensure that even on outputs which do not
    // have workspaces to show anything, a correct workspace is shown.
    let mut empty_outputs: HashSet<&String> = HashSet::from_iter(mappings.keys());
    // We want to now which workspace was focused to be able to focus it after
    // moving the workspaces.
    let mut focused_ws: Option<i32> = None;

    for ws in sway.get_workspaces().unwrap_or_default() {
        // Store the focused workspace
        if ws.focused {
            focused_ws = Some(ws.num);
        }

        for (output, workspaces) in mappings.iter() {
            // Skip output if it should not display the current workspace.
            if !workspaces.contains(&ws.num) {
                continue;
            }
            // We move a workspace to this output, remove it from the list of
            // empty outputs.
            empty_outputs.remove(output);
            // The workspace is already on the correct output, don't do anything.
            if &ws.output == output {
                break;
            }

            // 1. Select the workspace.
            // 2. Move the workspace to the desired output.
            sway.run_command(format!(
                "workspace --no-auto-back-and-forth number {}, move workspace to output '{}'",
                ws.num, output
            ))
            .expect("Cannot move workspace to output.");
        }
    }

    // Go through all outputs which have no workspace on them.
    for output in empty_outputs.into_iter() {
        // Get the first workspace in the assigned list of workspaces for the
        // output and display this workspace on the output.
        mappings
            .get(output)
            .and_then(|workspaces| workspaces.first())
            .map(|num| {
                sway.run_command(&format!("workspace --no-auto-back-and-forth number {num}, move workspace to output '{output}'"))
            });
    }

    // Focus the previously focused workspace.
    if let Some(ws) = focused_ws {
        sway.run_command(format!("workspace --no-auto-back-and-forth number {ws}"))
            .expect("Cannot switch back to focused workspace.");
    }
}
