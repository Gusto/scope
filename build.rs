use anyhow::Result;
use vergen::EmitBuilder;

pub fn main() -> Result<()> {
    let mut builder = EmitBuilder::builder();
    
    // Build info always works
    builder.all_build();
    
    // Git info only available when building from git repo
    // This allows the crate to be built from crates.io where .git doesn't exist
    if std::path::Path::new(".git").exists() {
        builder.all_git();
    } else {
        // Provide default values when not in a git repo
        println!("cargo:rustc-env=VERGEN_GIT_DESCRIBE=unknown");
        println!("cargo:rustc-env=VERGEN_GIT_SHA=unknown");
        println!("cargo:rustc-env=VERGEN_GIT_COMMIT_DATE=unknown");
    }
    
    builder.emit()?;
    Ok(())
}
