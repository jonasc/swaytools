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
    let config = make_config(cli.mapping, &mut sway);
    save_config(&config);
    move_workspaces(&config, &mut sway)
}

fn move_workspaces(mappings: &HashMap<String, Vec<i32>>, sway: &mut Connection) {
    let mut outputs: HashSet<&String> = HashSet::from_iter(mappings.keys());
    let mut focused_ws: Option<i32> = None;
    for ws in sway.get_workspaces().unwrap_or_default() {
        if ws.focused {
            focused_ws = Some(ws.num);
        }
        for (output, workspaces) in mappings.iter() {
            if !workspaces.contains(&ws.num) {
                continue;
            }
            outputs.remove(output);
            if &ws.output == output {
                break;
            }

            sway.run_command(format!(
                "workspace --no-auto-back-and-forth number {}, move workspace to output '{}'",
                ws.num, output
            ))
            .expect("Cannot move workspace to output.");
        }
    }
    for output in outputs.into_iter() {
        mappings
            .get(output)
            .and_then(|workspaces| workspaces.first())
            .map(|num| {
                sway.run_command(&format!(
                    "workspace --no-auto-back-and-forth number {}, move workspace to output '{}'",
                    num, output
                ))
            });
    }
    if let Some(ws) = focused_ws {
        sway.run_command(format!("workspace --no-auto-back-and-forth number {}", ws))
            .expect("Cannot switch back to focused workspace.");
    }
}
