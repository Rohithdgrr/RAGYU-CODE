// Simple test to validate git binary path logic
use std::path::PathBuf;

const TRUSTED_GIT_LOCATIONS: &[&str] = &[
    // Linux/Unix
    "/usr/bin/git",
    "/usr/local/bin/git",
    "/bin/git",
    // macOS Homebrew
    "/opt/homebrew/bin/git",
    // Windows
    "C:\\Program Files\\Git\\cmd\\git.exe",
    "C:\\Program Files (x86)\\Git\\cmd\\git.exe",
    "C:\\Program Files\\Git\\bin\\git.exe",
];

fn main() {
    println!("Testing git binary validation...");
    
    // Test 1: which crate can find git
    match which::which("git") {
        Ok(path) => {
            println!("✓ Found git at: {}", path.display());
            
            // Test 2: can canonicalize
            match path.canonicalize() {
                Ok(canonical) => {
                    println!("✓ Canonicalized to: {}", canonical.display());
                    
                    // Test 3: check if in trusted locations
                    let mut found = false;
                    for trusted in TRUSTED_GIT_LOCATIONS {
                        let trusted_path = PathBuf::from(trusted);
                        if let Ok(trusted_canonical) = trusted_path.canonicalize() {
                            if canonical == trusted_canonical {
                                println!("✓ Git is in trusted location: {}", trusted);
                                found = true;
                                break;
                            }
                        }
                    }
                    
                    if !found {
                        println!("✗ Git is NOT in a trusted location");
                        println!("  This is expected if git is installed in a non-standard location");
                    }
                }
                Err(e) => println!("✗ Failed to canonicalize: {}", e),
            }
        }
        Err(e) => println!("✗ Git not found in PATH: {}", e),
    }
    
    println!("\nTrusted locations:");
    for loc in TRUSTED_GIT_LOCATIONS {
        println!("  - {}", loc);
    }
}
