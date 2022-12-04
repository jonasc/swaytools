use swaytools::initialize_workspace;

fn main() {
    let (cli, mut sway, output, workspace_exists) = initialize_workspace();

    // If the workspace we want to go to already exists then we can just go there.
    // The only problem is that if the workspace does not exist yet, it will be created on the same output that is currently focused.
    // If the output where the workspace should be created is given, then we just focus this output first so that the workspace is created there.
    if !workspace_exists && output.is_some() {
        sway.run_command(format!("focus output {}", output.unwrap()))
            .expect("Cannot switch to output.");
    }
    // Create or switch to the desired workspace.
    sway.run_command(format!("workspace {}", cli.workspace))
        .expect("Cannot switch to workspace.");
}
