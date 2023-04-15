use swaytools::initialize_workspace;

fn main() {
    let (cli, mut sway, output, workspace_exists) = initialize_workspace();

    // Move the currently focused window to the workspace with the provided number.
    sway.run_command(format!("move to workspace number {}", cli.workspace))
        .expect("Cannot move window to workspace");

    // The only problem is that if the workspace does not exist yet, it will be created on the same output that is currently focused.
    // If the output where the workspace should be created is given, then we just move the workspace to this output.
    if !workspace_exists && output.is_some() {
        sway.run_command(format!(
            "[workspace={}] move workspace to '{}'",
            cli.workspace,
            output.unwrap()
        ))
        .expect("Cannot switch to output.");
    }

    // // Ensure that we have a focused workspace and an output the workspace to which we just moved the focused window should be put.
    // let Some(focused_workspace) = get_focused_workspace(&mut sway) else { return; };
    // let Some(output) = output else { return; };

    // // If the workspace already existed before moving the window there, it must be on the right output already.
    // // If otherwise the output is the same the focused one then the workspace will have been created on this output.
    // if workspace_exists || output == focused_workspace.output {
    //     return;
    // }
    // // Now we need to move the newly created workspace to its correct output.

    // // Obtain the workspace number of the visible workspace on the (unfocused) output to which the newly created workspace should be moved.
    // // We want this output to be visible afterwards again.
    // let visible_workspace_num = match get_visible_workspace_for_output(&output, &mut sway) {
    //     Some(workspace) => workspace.num,
    //     _ => focused_workspace.num,
    // };

    // // Now we move the newly created workspace to the desired output in the background.
    // // To do this we perform 4 steps:
    // // 1. Select the newly created workspace.
    // // 2. Move the newly created workspace to the desired output.
    // // 3. Focus the workspace which was visible on this output.
    // // 4. Focus the workspace which was focused before calling this function.
    // // FIXME: Step 4 breaks in the following situation: OUT1[], OUT2[WS2[_W_]]
    // // Step 0. OUT1[_WS3_], OUT2[WS1[W], _WS2_]
    // // Step 1. OUT1[_WS3_], OUT2[WS1[_W_]] // WS2 destroyed because it's empty and unselected
    // // Step 2. OUT1[WS3, WS1[_W_]], OUT2[_WS?_]
    // // Step 3. OUT1[_WS3_, WS1[W]], OUT2[_WS?_]
    // // Step 4. OUT1[WS3, WS1[W], _WS2_], OUT2[_WS?_]
    // sway.run_command(format!("workspace --no-auto-back-and-forth number {}, move workspace to output '{}', workspace --no-auto-back-and-forth number {}, workspace --no-auto-back-and-forth number {}", cli.workspace, output, visible_workspace_num, focused_workspace.num)).expect("Cannot move workspace to output.");
}
