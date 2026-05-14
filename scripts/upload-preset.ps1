param([string]$preset_path, [string]$osc_addr = '/electra1/preset/upload')
$exe = 'D:\Claude\osc-bridge\target\x86_64-pc-windows-gnu\release\osc-bridge.exe'
$json = [System.IO.File]::ReadAllText($preset_path)
# Invoke-Expression would re-interpret; use call operator with explicit array.
& $exe osc-send $osc_addr $json
