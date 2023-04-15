use swaytools::initialize_workspace;

fn main() {
    let (cli, mut sway, output, workspace_exists) = initialize_workspace();

    // If the workspace we want to go to already exists then we can just go there.
    // Create or switch to the desired workspace.
    println!("workspace {}", cli.workspace);
    sway.run_command(format!("workspace {}", cli.workspace))
        .expect("Cannot switch to workspace.");
    // The only problem is that if the workspace does not exist yet, it will be created on the same output that is currently focused.
    // If the output where the workspace should be created is given, then we just move the workspace to this output.
    if !workspace_exists && output.is_some() {
        println!(
            "[workspace={}] move workspace to '{}'",
            cli.workspace,
            output.to_owned().unwrap()
        );
        sway.run_command(format!(
            "[workspace={}] move workspace to '{}'",
            cli.workspace,
            output.unwrap()
        ))
        .expect("Cannot switch to output.");
    }
}
