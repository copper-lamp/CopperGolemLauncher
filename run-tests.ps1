. '.cargo\env-check.ps1'
cargo test --workspace *>&1 | Tee-Object -FilePath 'target\test-run.log'
$code = $LASTEXITCODE
Write-Output "EXIT=$code"
exit $code
