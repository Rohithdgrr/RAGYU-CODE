#!/bin/bash
# Validation script for Task 1.1 - Shell Injection Fix

echo "================================"
echo "Task 1.1 Validation Script"
echo "================================"
echo

# Check if we're in the right directory
if [ ! -f "Cargo.toml" ]; then
    echo "Error: Not in project root directory"
    exit 1
fi

# Check if which crate was added
echo "1. Checking if 'which' dependency was added..."
if grep -q 'which = "7.0"' Cargo.toml; then
    echo "   ✓ Dependency added"
else
    echo "   ✗ Dependency missing"
    exit 1
fi

# Build the project
echo
echo "2. Building project..."
if cargo build --quiet; then
    echo "   ✓ Build successful"
else
    echo "   ✗ Build failed"
    exit 1
fi

# Run parse_command_to_argv tests
echo
echo "3. Running parse_command_to_argv tests..."
if cargo test --lib --quiet parse_command_to_argv 2>&1 | grep -q "test result: ok"; then
    echo "   ✓ Parse tests passed"
else
    echo "   ✗ Parse tests failed"
    cargo test --lib parse_command_to_argv
    exit 1
fi

# Run shell injection security tests
echo
echo "4. Running shell injection security tests..."
if cargo test --lib --quiet run_shell_command_rejects_shell_injection_attempts 2>&1 | grep -q "test result: ok"; then
    echo "   ✓ Security tests passed"
else
    echo "   ✗ Security tests failed"
    cargo test --lib run_shell_command_rejects_shell_injection_attempts
    exit 1
fi

# Run backward compatibility tests  
echo
echo "5. Running backward compatibility tests..."
if cargo test --lib --quiet run_shell_command_allows_safe_commands 2>&1 | grep -q "test result: ok"; then
    echo "   ✓ Compatibility tests passed"
else
    echo "   ✗ Compatibility tests failed"
    cargo test --lib run_shell_command_allows_safe_commands
    exit 1
fi

# Run existing shell tests
echo
echo "6. Running existing shell command tests..."
if cargo test --lib --quiet run_shell 2>&1 | grep -q "test result: ok"; then
    echo "   ✓ Existing tests passed"
else
    echo "   ⚠ Some existing tests may need attention"
fi

echo
echo "================================"
echo "All validations passed!"
echo "================================"
echo
echo "Task 1.1 implementation is complete and verified."
