-- Yggdrasil's session on Hyprland (started by yggdrasil-session). Hyprland draws and moves the windows; the rest of the desktop is
-- Yggdrasil's own programs, started from the standard autostart folder. Windows float, like on Windows and macOS (Super+T tiles one).
--
-- Your own additions and changes: ~/.config/yggdrasil/hyprland.lua (read last, so it wins). Monitors: the same file, e.g.
--   hl.monitor({ output = "DP-1", mode = "3440x1440@180", position = "0x0", scale = 1 })

local home = os.getenv("HOME") or ""
local config_home = os.getenv("XDG_CONFIG_HOME") or (home .. "/.config")

-- A value from one of Yggdrasil's own small settings files ("key = value" lines).
local function setting(file, key)
  local f = io.open(config_home .. "/yggdrasil/" .. file, "r")
  if not f then return nil end
  for line in f:lines() do
    local k, v = line:match("^%s*([%w_]+)%s*=%s*([^#]-)%s*$")
    if k == key then f:close() return v end
  end
  f:close()
  return nil
end

-- The accent (Settings > Appearance) for the focused window's border; Yggdrasil's purple when none is chosen.
local accent = "7c54d6"
do
  local r, g, b = (setting("appearance.conf", "accent") or ""):match("^(%d+)%s*,%s*(%d+)%s*,%s*(%d+)$")
  if r then accent = string.format("%02x%02x%02x", tonumber(r), tonumber(g), tonumber(b)) end
end
local dark = setting("appearance.conf", "style") ~= "light"
local reduce_motion = setting("panel.conf", "reduce_motion") == "yes" or setting("panel.conf", "reduce_motion") == "true"

-- Every monitor at its fastest mode until you set your own (see above).
hl.monitor({ output = "", mode = "highrr", position = "auto", scale = 1 })

hl.config({
  input = {
    kb_layout = "us",
    -- click to focus, as on Windows and macOS (moving the pointer alone does not change the focused window)
    follow_mouse = 2,
  },

  general = {
    gaps_in = 4,
    gaps_out = 8,
    border_size = 1,
    col = {
      active_border = "rgba(" .. accent .. "cc)",
      inactive_border = dark and "rgba(ffffff14)" or "rgba(00000018)",
    },
    layout = "dwindle",
    -- drag a window's edge to resize it
    resize_on_border = true,
    extend_border_grab_area = 12,
    -- a game that asks for it may show frames the moment they are ready (lowest input lag); nothing tears otherwise
    allow_tearing = true,
  },

  decoration = {
    rounding = 10,
    shadow = { enabled = true, range = 18, render_power = 3, color = "rgba(00000044)" },
    -- no blur: it costs the graphics card something on every frame
    blur = { enabled = false },
  },

  animations = {
    enabled = not reduce_motion,
  },

  misc = {
    disable_hyprland_logo = true,
    disable_splash_rendering = true,
    focus_on_activate = true,
    -- variable refresh for full-screen programs (games), never for the desktop (no flicker)
    vrr = 2,
  },

  cursor = {
    hide_on_key_press = false,
  },
})

-- Windows float, centred, at the size the program asks for.
hl.window_rule({ name = "windows-float", match = { class = ".*" }, float = true, center = true })

-- Shortcuts (the same table as Yggdrasil's own compositor, compositor/src/shortcuts.rs).
local function cmd(keys, command, opts) hl.bind(keys, hl.dsp.exec_cmd(command), opts) end
local function dsp(keys, dispatcher, opts) hl.bind(keys, dispatcher, opts) end

cmd("SUPER + SUPER_L", "yggdrasil-panelctl start", { release = true, desc = "Start menu" })
cmd("SUPER + S", "yggdrasil-panelctl ask-press", { desc = "Talk to the assistant (hold)" })
cmd("SUPER + S", "yggdrasil-panelctl ask-release", { release = true, desc = "Talk to the assistant (let go)" })
cmd("CTRL + ALT + S", "yggdrasil-panelctl ask-press", { desc = "Talk to the assistant (hold)" })
cmd("CTRL + ALT + S", "yggdrasil-panelctl ask-release", { release = true, desc = "Talk to the assistant (let go)" })
cmd("SUPER + V", "yggdrasil-panelctl clipboard", { desc = "Clipboard history" })
cmd("SUPER + C", "yggdrasil-panelctl calendar", { desc = "Calendar" })
cmd("SUPER + N", "yggdrasil-panelctl notifications", { desc = "Notifications" })
cmd("SUPER + E", "minos", { desc = "Files (Minos)" })
cmd("SUPER + I", "yggdrasil-settings", { desc = "Settings" })
cmd("SUPER + RETURN", "ghostty", { desc = "Terminal" })
cmd("SUPER + L", "loginctl lock-session", { desc = "Lock the screen" })
cmd("CTRL + SHIFT + ESCAPE", "ghostty -e btop", { desc = "Running programs" })

dsp("ALT + F4", hl.dsp.window.close(), { desc = "Close the window" })
dsp("SUPER + Q", hl.dsp.window.close(), { desc = "Close the window" })
dsp("SUPER + F", hl.dsp.window.fullscreen({ mode = "fullscreen" }), { desc = "Full screen" })
dsp("SUPER + UP", hl.dsp.window.fullscreen({ mode = "maximized" }), { desc = "Maximize (again: restore)" })
dsp("SUPER + DOWN", hl.dsp.window.move({ workspace = "special:minimized", follow = false }), { desc = "Minimize" })
dsp("SUPER + SHIFT + M", hl.dsp.workspace.toggle_special("minimized"), { desc = "Show minimized windows" })
cmd("SUPER + LEFT", "yggdrasil-snap left", { desc = "Left half" })
cmd("SUPER + RIGHT", "yggdrasil-snap right", { desc = "Right half" })
dsp("SUPER + T", hl.dsp.window.float({ action = "toggle" }), { desc = "Tile or float the window" })
dsp("ALT + TAB", hl.dsp.window.cycle_next(), { desc = "Next window" })
dsp("ALT + TAB", hl.dsp.window.bring_to_top())
dsp("ALT + SHIFT + TAB", hl.dsp.window.cycle_next({ next = false }), { desc = "Previous window" })
dsp("ALT + SHIFT + TAB", hl.dsp.window.bring_to_top())

-- Workspaces (desktops), as on Windows: Super+Ctrl+Left/Right, or a number; with Shift the window goes along.
dsp("SUPER + CTRL + LEFT", hl.dsp.focus({ workspace = "r-1" }), { desc = "Previous desktop" })
dsp("SUPER + CTRL + RIGHT", hl.dsp.focus({ workspace = "r+1" }), { desc = "Next desktop" })
for n = 1, 4 do
  local key = "code:" .. tostring(n + 9)
  dsp("SUPER + CTRL + " .. key, hl.dsp.focus({ workspace = tostring(n) }), { desc = "Desktop " .. n })
  dsp("SUPER + CTRL + SHIFT + " .. key, hl.dsp.window.move({ workspace = tostring(n) }), { desc = "Move the window to desktop " .. n })
end

-- The mouse: Super and drag to move a window, Super and right-drag to resize it.
dsp("SUPER + mouse:272", hl.dsp.window.drag(), { mouse = true, desc = "Move the window" })
dsp("SUPER + mouse:273", hl.dsp.window.resize(), { mouse = true, desc = "Resize the window" })

-- Screenshots: a region to the clipboard and to Pictures/Screenshots.
cmd("PRINT", "sh -c 'd=\"$(xdg-user-dir PICTURES)/Screenshots\"; mkdir -p \"$d\"; f=\"$d/Screenshot $(date +%F\\ %H-%M-%S).png\"; grim -g \"$(slurp)\" \"$f\" && wl-copy < \"$f\" && notify-send -i \"$f\" Screenshot \"Saved and copied\"'", { desc = "Screenshot of a region" })

-- The multimedia keys, also on the lock screen.
for key, what in pairs({
  XF86AudioRaiseVolume = "volume-up", XF86AudioLowerVolume = "volume-down", XF86AudioMute = "mute", XF86AudioMicMute = "mic-mute",
  XF86AudioPlay = "play-pause", XF86AudioPause = "play-pause", XF86AudioNext = "next", XF86AudioPrev = "previous", XF86AudioStop = "stop",
  XF86MonBrightnessUp = "brightness-up", XF86MonBrightnessDown = "brightness-down",
}) do
  local repeating = what:find("volume") ~= nil or what:find("brightness") ~= nil
  cmd(key, "yggdrasil-mediakey " .. what, { locked = true, repeating = repeating })
end

-- The session's programs: the environment goes to systemd and D-Bus first (so programs started there find this display), then the
-- autostart folder (the panel, the idle watch, the password box, the keyring check ...). Programs meant for other desktops only
-- (OnlyShowIn=KDE, ...) are left out by the autostart rules themselves.
hl.on("hyprland.start", function()
  hl.exec_cmd("sh -c 'dbus-update-activation-environment --systemd WAYLAND_DISPLAY DISPLAY XDG_CURRENT_DESKTOP XDG_SESSION_DESKTOP XDG_SESSION_TYPE HYPRLAND_INSTANCE_SIGNATURE QT_QPA_PLATFORM GDK_BACKEND MOZ_ENABLE_WAYLAND ELECTRON_OZONE_PLATFORM_HINT LIBVA_DRIVER_NAME __GLX_VENDOR_LIBRARY_NAME NVD_BACKEND && systemctl --user start xdg-desktop-autostart.target'")
end)

-- Yours last.
local user_file = config_home .. "/yggdrasil/hyprland.lua"
local f = io.open(user_file, "r")
if f then
  f:close()
  local ok, err = pcall(dofile, user_file)
  if not ok then io.stderr:write(("yggdrasil: your hyprland.lua has a mistake: %s\n"):format(err)) end
end
