Output assets/demo.gif

Set Shell "bash"
Set FontFamily "CaskaydiaMono Nerd Font"
Set FontSize 15
Set Width 1110
Set Height 560
Set Framerate 30
Set CursorBlink false
Set TypingSpeed 60ms
Set Padding 0

# Cypherpunk green-on-black theme matching the padctl TUI
Set Theme {"name":"padctl","background":"#08080c","foreground":"#00e6a0","selection":"#00e6a022","cursor":"#00e6a0","black":"#08080c","red":"#dc3c3c","green":"#00e6a0","yellow":"#ffb900","blue":"#00d2dc","magenta":"#ff50c8","cyan":"#00d2dc","white":"#b4b4b4","bright_black":"#404040","bright_red":"#ff6464","bright_green":"#78ffc0","bright_yellow":"#ffd060","bright_blue":"#60e8ff","bright_magenta":"#ff80e8","bright_cyan":"#80f0ff","bright_white":"#ffffff"}

# Hide shell bootstrap — recording starts from the TUI
Hide
Type "bash demo/padctl-demo.sh"
Enter
Sleep 800ms
Show

# Phase 1: Scanning
Sleep 1.5s

# Phase 1b: Device appears + auto-connect
Sleep 2s

# Phase 2: Connecting
Sleep 2s

# Phase 3: Session idle
Sleep 1.2s

# Phase 4: Running at 3.0 km/h (8 ticks)
Sleep 4.4s

# Phase 5: Speed ramp to 4.0 km/h + a few more ticks
Sleep 4s

# Phase 6: Paused
Sleep 2.2s

# Phase 7: Resume + more ticks
Sleep 2.2s

# Phase 8: End session
Sleep 0.8s

# Phase 9: Summary — hold for 5s then stop recording
Sleep 5s
