@echo off
echo Running git module tests...
cargo test --lib git::tests >test_output.txt 2>&1
set TEST_EXIT=%ERRORLEVEL%

echo.
echo Running security tests...
cargo test --test git_security_test >>test_output.txt 2>&1
set SEC_EXIT=%ERRORLEVEL%

if %TEST_EXIT% EQU 0 if %SEC_EXIT% EQU 0 (
    echo ALL TESTS PASSED
    type test_output.txt
    exit /b 0
) else (
    echo SOME TESTS FAILED
    type test_output.txt
    exit /b 1
)
