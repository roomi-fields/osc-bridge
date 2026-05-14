param([string]$preset, [int]$bank = 0, [int]$slot = 0)
$exe = 'D:\Claude\osc-bridge\target\x86_64-pc-windows-gnu\release\osc-bridge.exe'
# 1. Target the slot explicitly before upload
& $exe osc-send /electra1/preset/arm_upload $bank $slot
Start-Sleep -Milliseconds 100
# 2. Upload the JSON
& $exe osc-send /electra1/preset/upload --from-file $preset
Start-Sleep -Milliseconds 500
# 3. Force-switch to the slot we just wrote to
& $exe osc-send /electra1/preset/switch $bank $slot
