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

-- Every monitor at its fastest mode, until Settings > Displays (or you) sets them: ~/.config/yggdrasil/monitors.lua.
hl.monitor({ output = "", mode = "highrr", position = "auto", scale = 1 })
do
  local monitors = config_home .. "/yggdrasil/monitors.lua"
  local f = io.open(monitors, "r")
  if f then
    f:close()
    local ok, err = pcall(dofile, monitors)
    if not ok then io.stderr:write(("yggdrasil: monitors.lua has a mistake: %s\n"):format(err)) end
  end
end

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
    -- no reloading by itself when this file changes: an update replaces it, and a reload caught in between found no file and left
    -- the session without its settings. Settings reloads after its changes; after editing your own file: hyprctl reload
    disable_autoreload = true,
    -- if the lock screen ever crashes, a new one may take over the locked screen (yggdrasil-idle starts it again) instead of
    -- Hyprland's "lock screen died" warning staying up
    allow_session_lock_restore = true,
  },

  cursor = {
    hide_on_key_press = false,
  },

  debug = {
    -- config mistakes go to the notification center (the panel asks after every load), not into a box over the screen
    suppress_errors = true,
  },

  render = {
    -- a full-screen game's frames go straight to the monitor, not through the compositor (less input delay); games only
    direct_scanout = 2,
  },
})

-- Windows float, centred, at the size the program asks for...
hl.window_rule({ name = "windows-float", match = { class = ".*" }, float = true, center = true })

-- Steam asks at start to have its window un-maximized, and Hyprland 0.56 maximizes it instead: its maximize requests are
-- ignored (Super+Up still maximizes it)
hl.window_rule({ name = "steam-no-self-maximize", match = { class = "^(steam)$" }, suppress_event = "maximize" })

-- Steam games are marked as games (for the direct path to the monitor above)
hl.window_rule({ name = "steam-games", match = { class = "^(steam_app_.*)$" }, content = "game" })

-- ...except on the screens set to tile (Settings > Displays; ~/.config/yggdrasil/tiling, one screen per line; before that file exists,
-- the choice made under KDE). There windows sit side by side with gaps, Omarchy-style. Settings reloads this when it changes.
local function tiling_screens()
  local list = {}
  local f = io.open(config_home .. "/yggdrasil/tiling", "r")
  if f then
    for line in f:lines() do
      for name in (line:gsub("#.*", "")):gmatch("[%w%-_%.]+") do table.insert(list, name) end
    end
    f:close()
    return list
  end
  local k = io.open(config_home .. "/kwinrc", "r")
  if k then
    local in_group = false
    for line in k:lines() do
      if line:match("^%[") then in_group = (line == "[Script-labyrinth-screen-keys]")
      elseif in_group then
        local v = line:match("^tilingOutputs=(.*)$")
        if v then for name in v:gmatch("[%w%-_%.]+") do table.insert(list, name) end end
      end
    end
    k:close()
  end
  return list
end
for _, screen in ipairs(tiling_screens()) do
  hl.window_rule({ name = "tile-on-" .. screen, match = { class = ".*", workspace = "m[" .. screen .. "]" }, tile = true })
end

-- Windows come back where they were: when a program's last window closes, its size and place are kept (one line per program in
-- ~/.local/state/yggdrasil/window-places), and its next window opens there, if that spot is still on a screen. Dialogs and second
-- windows of a program that already has one open are left where the program puts them.
local places_file = (os.getenv("XDG_STATE_HOME") or (home .. "/.local/state")) .. "/yggdrasil/window-places"
local places = {}
do
  local f = io.open(places_file, "r")
  if f then
    for line in f:lines() do
      local class, x, y, w, h = line:match("^(%S+)\t(%-?%d+)\t(%-?%d+)\t(%d+)\t(%d+)$")
      if class then places[class] = { tonumber(x), tonumber(y), tonumber(w), tonumber(h) } end
    end
    f:close()
  end
end
local function xy(v)
  if type(v) == "table" then return v.x or v[1], v.y or v[2] end
  return nil, nil
end
local function others_of(w)
  local n = 0
  for _, o in ipairs(hl.get_windows()) do
    if o.address ~= w.address and o.class == w.class then n = n + 1 end
  end
  return n
end
hl.on("window.close", function(w)
  local ok, err = pcall(function()
    if not w.floating or (w.fullscreen or 0) ~= 0 or w.class == "" or others_of(w) > 0 then return end
    local ws = w.workspace
    if ws and ws.name and ws.name:sub(1, 8) == "special:" then return end
    local x, y = xy(w.at)
    local width, height = xy(w.size)
    if not x or not width or width < 120 or height < 80 then return end
    places[w.class] = { x, y, width, height }
    os.execute("mkdir -p '" .. places_file:match("^(.*)/") .. "'")
    local f = io.open(places_file .. ".tmp", "w")
    if not f then return end
    for class, p in pairs(places) do f:write(string.format("%s\t%d\t%d\t%d\t%d\n", class, p[1], p[2], p[3], p[4])) end
    f:close()
    os.rename(places_file .. ".tmp", places_file)
  end)
  if not ok then io.stderr:write("yggdrasil: keeping a window's place: " .. tostring(err) .. "\n") end
end)
hl.on("window.open", function(w)
  local ok, err = pcall(function()
    local p = places[w.class]
    if not p or not w.floating or others_of(w) > 0 then return end
    if not hl.get_monitor_at(p[1] + p[3] / 2, p[2] + p[4] / 2) then return end
    local target = "address:" .. w.address
    -- resizing keeps a window centred, so the size first, then the place
    hl.dispatch(hl.dsp.window.resize({ x = p[3], y = p[4], window = target }))
    hl.dispatch(hl.dsp.window.move({ x = p[1], y = p[2], window = target }))
  end)
  if not ok then io.stderr:write("yggdrasil: putting a window back: " .. tostring(err) .. "\n") end
end)

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

cmd("ALT + F4", "yggdrasil-close", { desc = "Close the window (programs that keep running: hide it)" })
cmd("SUPER + Q", "yggdrasil-close", { desc = "Close the window (programs that keep running: hide it)" })
dsp("SUPER + F", hl.dsp.window.fullscreen({ mode = "fullscreen" }), { desc = "Full screen" })
cmd("SUPER + UP", "yggdrasil-snap up", { desc = "Maximize (again: restore), or focus up on a tiling screen" })
cmd("SUPER + DOWN", "yggdrasil-snap down", { desc = "Minimize, or focus down on a tiling screen" })
cmd("SUPER + W", "yggdrasil-close", { desc = "Close the window (Omarchy)" })
dsp("SUPER + SHIFT + M", hl.dsp.workspace.toggle_special("minimized"), { desc = "Show minimized windows" })
-- Super+Arrows: on a floating screen, snap the window (Windows); on a tiling screen, move the focus (Omarchy)
cmd("SUPER + LEFT", "yggdrasil-snap left", { desc = "Left half, or focus left on a tiling screen" })
cmd("SUPER + RIGHT", "yggdrasil-snap right", { desc = "Right half, or focus right on a tiling screen" })
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
  -- each step on its own (one failing must not stop the next), each written to the system log under "yggdrasil-session"
  hl.exec_cmd("sh -c 'systemctl --user import-environment WAYLAND_DISPLAY DISPLAY XDG_CURRENT_DESKTOP XDG_SESSION_DESKTOP XDG_SESSION_TYPE HYPRLAND_INSTANCE_SIGNATURE QT_QPA_PLATFORM QT_WAYLAND_DISABLED_INTERFACES QT_QPA_PLATFORMTHEME MANGOHUD_CONFIGFILE GDK_BACKEND MOZ_ENABLE_WAYLAND ELECTRON_OZONE_PLATFORM_HINT LIBVA_DRIVER_NAME __GLX_VENDOR_LIBRARY_NAME NVD_BACKEND 2>&1 | systemd-cat -t yggdrasil-session; dbus-update-activation-environment --systemd --all 2>&1 | systemd-cat -t yggdrasil-session; systemctl --user start yggdrasil-session.target 2>&1 | systemd-cat -t yggdrasil-session; echo autostart started for $XDG_CURRENT_DESKTOP on $WAYLAND_DISPLAY | systemd-cat -t yggdrasil-session'")
end)

-- Yours last.
local user_file = config_home .. "/yggdrasil/hyprland.lua"
local f = io.open(user_file, "r")
if f then
  f:close()
  local ok, err = pcall(dofile, user_file)
  if not ok then io.stderr:write(("yggdrasil: your hyprland.lua has a mistake: %s\n"):format(err)) end
end
