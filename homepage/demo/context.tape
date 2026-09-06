# Capture the Context page from the same isolated fixture as the workflow demo.
Output "/out/context-capture.mp4"
Set Shell bash
Set FontFamily "DejaVu Sans Mono"
Set FontSize 20
Set Width 1488
Set Height 856
Set Padding 16
Set CursorBlink false
Set Theme {"name":"gruvbox-dark","background":"#282828","foreground":"#ebdbb2","cursor":"#ebdbb2","selection":"#504945","black":"#282828","red":"#cc241d","green":"#98971a","yellow":"#d79921","blue":"#458588","magenta":"#b16286","cyan":"#689d6a","white":"#a89984","brightBlack":"#928374","brightRed":"#fb4934","brightGreen":"#b8bb26","brightYellow":"#fabd2f","brightBlue":"#83a598","brightMagenta":"#d3869b","brightCyan":"#8ec07c","brightWhite":"#ebdbb2"}
Env MDMANAGER_THEME "gruvbox-dark"
Hide
Type "bash /demo/fixture.sh && cd ~/projects/harbor && mdmanager"
Enter
Sleep 5s
Enter
Sleep 2s
Down
Sleep 1s
Show
Sleep 500ms
Screenshot "/out/context-rendered.png"
