fn forbidden_direct_process_wait(mut child: std::process::Child) {
    let _ = child.wait();
}
