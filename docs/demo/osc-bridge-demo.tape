# osc-bridge demo — generates osc-bridge-demo.gif
# Run from the repo root: vhs docs/demo/osc-bridge-demo.tape

Output docs/demo/osc-bridge-demo.gif

Set FontSize 16
Set Width 1280
Set Height 760
Set Padding 24
Set TypingSpeed 28ms
Set Theme "Catppuccin Mocha"

# Setup (hidden): cd to the repo, put the release binary on PATH.
Hide
Type "cd /home/romi/dev/music/osc-bridge && export PATH=$PWD/target/release:$PATH && clear"
Enter
Sleep 500ms
Show

# --- Beat 1: identity ---
Type "# osc-bridge — OSC <-> MIDI bridge for 849 synths & DAWs, plus an MCP server"
Sleep 1.5s
Enter
Sleep 500ms

# --- Beat 2: the tool surface ---
Type "osc-bridge --help"
Sleep 300ms
Enter
Sleep 2.8s

# --- Beat 3: a hardware synth's named OSC surface ---
Type "# a hardware synth — named OSC routes, no protocol hand-rolling:"
Enter
Sleep 400ms
Type "osc-bridge inspect devices/arturia/minilab3.json"
Sleep 300ms
Enter
Sleep 3.2s

# --- Beat 4: a software target (DAW) ---
Type "# the same surface for software — here Ableton Live over OSC:"
Enter
Sleep 400ms
Type "osc-bridge inspect devices/ableton/live.third-party-osc.fw-12.2.json"
Sleep 300ms
Enter
Sleep 3.2s

# --- Beat 5: the MCP server ---
Type "# ...and an MCP server — a MIDI MCP and OSC MCP. Its tools:"
Enter
Sleep 400ms
Type "bash docs/demo/mcp-tools.sh"
Sleep 300ms
Enter
Sleep 2.8s

# --- Beat 6: tagline ---
Type "# 849 drivers - hardware + software - MCP - GPL-3"
Enter
Type "# npx -y @roomi-fields/osc-bridge mcp"
Sleep 3.5s
Enter
