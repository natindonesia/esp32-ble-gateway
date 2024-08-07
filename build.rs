fn get_git_hash() -> String {
    let output = std::process::Command::new("git")
        .args(&["rev-parse", "--short", "HEAD"])
        .output()
        .expect("Failed to execute git command");
    let hash = String::from_utf8(output.stdout).unwrap();
    hash.trim().to_string()
}

fn get_git_commit_datetime() -> String {
    let output = std::process::Command::new("git")
        .args(&["show", "-s", "--format=%ci"])
        .output()
        .expect("Failed to execute git command");
    let datetime = String::from_utf8(output.stdout).unwrap();
    datetime.trim().to_string()
}

fn main() {
    let git_hash = get_git_hash();
    let git_commit_datetime = get_git_commit_datetime();
    println!("cargo:rustc-env=GIT_HASH={}", git_hash);
    println!("cargo:rustc-env=GIT_COMMIT_DATETIME={}", git_commit_datetime);
    embuild::espidf::sysenv::output();
}
