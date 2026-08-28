@echo off
echo Building project...
cargo build >build_output.txt 2>&1
if %ERRORLEVEL% EQU 0 (
    echo BUILD SUCCESS
    type build_output.txt
    exit /b 0
) else (
    echo BUILD FAILED - Exit code: %ERRORLEVEL%
    type build_output.txt
    exit /b 1
)
