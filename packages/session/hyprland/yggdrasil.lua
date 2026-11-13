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
-- Settings > Accessibility > Colour filters: a screen shader Settings writes as ~/.config/yggdrasil/shaders/NAME.frag, named by
-- `color_filter` in accessibility.conf (greyscale, protanopia, deuteranopia, tritanopia; none or missing: no shader)
local screen_shader = nil
do
  local name = setting("accessibility.conf", "color_filter")
  if name and name ~= "none" and name:match("^%a+$") then
    local p = config_home .. "/yggdrasil/shaders/" .. name .. ".frag"
    local f = io.open(p, "r")
    if f then
      f:close()
      screen_shader = p
    end
  end
end

-- The keyboard layout the system was set up with (the installer, or `localectl set-x11-keymap`), from the file both write:
-- /etc/X11/xorg.conf.d/00-keyboard.conf. US when there is none.
local function system_keyboard()
  local layout, variant, options = "us", "", ""
  local f = io.open("/etc/X11/xorg.conf.d/00-keyboard.conf", "r")
  if f then
    for line in f:lines() do
      local k, v = line:match('^%s*Option%s+"(Xkb%a+)"%s+"([^"]*)"')
      if k == "XkbLayout" and v ~= "" then layout = v
      elseif k == "XkbVariant" then variant = v
      elseif k == "XkbOptions" then options = v end
    end
    f:close()
  end
  return layout, variant, options
end
local kb_layout, kb_variant, kb_options = system_keyboard()

-- Monitors. Until Settings > Displays (or you) sets one, a monitor comes up at its own resolution (the one it names itself, its
-- "preferred" mode) at the fastest refresh rate it has for that resolution: an X34 at 3440x1440@180, an LG DualUp at 2560x2880.
-- Hyprland has no word for that: "highrr" is the fastest mode of any size (2026-11-13 the DualUp came up at 800x600, whose
-- 60.32 Hz beats the 59.98 Hz of its own size), "highres" the biggest one (the 3840x2160 an X34 accepts and scales down, at
-- 60 Hz), and "preferred" its own mode at the rate it names (60 Hz on the X34). So "preferred" is the rule for every screen,
-- and each screen there is gets a rule of its own with the fastest rate at that size, worked out from its modes (the end of
-- this file: the screens there are now, and a screen added later, plugged in or switched on, the moment Hyprland adds it).
-- Scale 1, except a laptop's own screen (eDP, or LVDS on older ones): its scale is Hyprland's automatic one, from the panel's
-- size and resolution, so a sharp 14-inch panel is not tiny (2026-11-12). Monitors on a cable stay at 1.
-- A screen monitors.lua (Settings > Displays) or your hyprland.lua names keeps what it says: nothing is worked out for it.
hl.monitor({ output = "", mode = "preferred", position = "auto", scale = 1 })
-- the screens monitors.lua and your hyprland.lua name: their `output`, a connector ("DP-1") or "desc:" and the start of the
-- monitor's description; "" (a rule for every screen) means you set them all
local named_screens = {}
-- runs one of those files, writing down the screens it names
local function run_with_monitors(path, what)
  local f = io.open(path, "r")
  if not f then return end
  f:close()
  local monitor = hl.monitor
  hl.monitor = function(spec, ...)
    if type(spec) == "table" and type(spec.output) == "string" then named_screens[spec.output] = true end
    return monitor(spec, ...)
  end
  local ok, err = pcall(dofile, path)
  hl.monitor = monitor
  if not ok then io.stderr:write(("yggdrasil: %s has a mistake: %s\n"):format(what, err)) end
end
run_with_monitors(config_home .. "/yggdrasil/monitors.lua", "monitors.lua")

hl.config({
  input = {
    kb_layout = kb_layout,
    kb_variant = kb_variant,
    kb_options = kb_options,
    -- click to focus, as on Windows and macOS (moving the pointer alone does not change the focused window)
    follow_mouse = 2,
    -- a touchpad scrolls the way the content moves, as on Windows and macOS (Hyprland's default is the reverse)
    touchpad = { natural_scroll = true },
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
    screen_shader = screen_shader,
  },

  animations = {
    enabled = not reduce_motion,
  },

  misc = {
    disable_hyprland_logo = true,
    -- before anything is drawn the screen is the same dark purple as the screen after signing in, so it fades in from that
    background_color = "rgb(05030a)",
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
    -- XDG_CURRENT_DESKTOP is Yggdrasil on purpose (programs and portals follow it): no warning about it being set from outside
    disable_xdg_env_checks = true,
    -- yggdrasil-session is Hyprland's watchdog (it restarts it with these settings after a crash, not in safe mode)
    disable_watchdog_warning = true,
  },

  cursor = {
    hide_on_key_press = false,
    -- the pointer moves only when you move it, as on Windows and macOS. Hyprland warps it to the middle of a window that is
    -- focused by a command, and the keyboard hand-back below focuses the front window whenever a pop-up closes
    -- (notifications close on their own), so the pointer jumped to the middle of the screen at random (2026-11-13).
    no_warps = true,
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

-- A laptop's own touchscreen and pen draw on its own screen (eDP), not across every screen once a monitor is plugged in.
-- Touchscreens are a laptop's (or a tablet's) own; of the pens, only those built into the screen (Wacom's HID digitisers and
-- the ELAN, Goodix and other stylus panels), never a drawing tablet on a cable such as an Intuos or a Huion (2026-11-13).
do
  local panel
  for _, m in ipairs(hl.get_monitors() or {}) do
    if tostring(m.name):sub(1, 3) == "eDP" then panel = tostring(m.name) end
  end
  if panel then
    for _, d in ipairs(hl.get_devices() or {}) do
      local name = tostring(d.name)
      local builtin_pen = d.type == "tablet"
        and (name:sub(1, 10) == "wacom-hid-" or name:find("elan", 1, true) or name:find("goodix", 1, true) or name:find("stylus", 1, true))
      if d.type == "touch" or builtin_pen then
        hl.device({ name = name, output = panel })
      end
    end
  end
end

-- A 2-in-1's tablet-mode switch (the screen folded back): /usr/lib/yggdrasil/tablet (Settings' package) then turns the built-in
-- screen with the accelerometer, its touchscreen and pen with it, and shows the on-screen keyboard while no keyboard is plugged
-- in (2026-11-13, ThinkPad X1 Yoga). Hyprland hands a switch's changes to binds named after the switch: the ones it lists, and
-- the kernel's names for the usual tablet-mode switches (thinkpad_acpi, intel-hid, intel-vbtn, GPIO), since a bind for a switch
-- that is not there does nothing. The lid's switch is logind's.
do
  local tablet = "/usr/lib/yggdrasil/tablet"
  local f = io.open(tablet, "r")
  if f then
    f:close()
    local names, seen = {}, {}
    local function add(n)
      n = tostring(n)
      if n ~= "" and not seen[n] and not n:lower():find("lid", 1, true) then seen[n] = true table.insert(names, n) end
    end
    for _, d in ipairs(hl.get_devices() or {}) do
      if d.type == "switch" then add(d.name) end
    end
    for _, n in ipairs({ "ThinkPad Extra Buttons", "Intel HID switches", "Intel Virtual Switches", "Tablet Mode Switch", "GPIO Tablet Mode Switch" }) do add(n) end
    for _, n in ipairs(names) do
      pcall(hl.bind, "switch:on:" .. n, hl.dsp.exec_cmd(tablet .. " mode on"), { locked = true, desc = "Tablet mode" })
      pcall(hl.bind, "switch:off:" .. n, hl.dsp.exec_cmd(tablet .. " mode off"), { locked = true, desc = "Laptop mode" })
    end
  end
end

-- A ThinkPad's TrackPoint scrolls while its middle button is held (a middle press without moving is still a middle click).
-- Its buttons can arrive on the built-in touchpad's companion "-mouse" device, so that one gets it too; other mice do not.
-- Without this the middle button only clicked (ThinkPad X1 Yoga, 2026-11-13).
do
  local devices = hl.get_devices() or {}
  local touchpads = {}
  for _, d in ipairs(devices) do
    local name = tostring(d.name)
    if name:sub(-9) == "-touchpad" then touchpads[name:sub(1, -10)] = true end
  end
  for _, d in ipairs(devices) do
    local name = tostring(d.name)
    if d.type == "pointer" and (name:find("trackpoint", 1, true) or (name:sub(-6) == "-mouse" and touchpads[name:sub(1, -7)])) then
      hl.device({ name = name, scroll_method = "on_button_down", scroll_button = 274 })
    end
  end
end

-- The mouse and keyboard (Settings > Peripherals; ~/.config/yggdrasil/input.conf, its form in packages/wm/src/input.rs): pointer
-- speed, acceleration, scrolling, left-handed and key repeat for every mouse and keyboard, and a mouse's own settings in a
-- [name] section (given to its other parts, name-1 to name-9, too: Hyprland lists a mouse's extra buttons as more devices). A
-- line that is not understood is left out. Settings gives a change to the running session at once; this keeps it.
local function input_settings()
  local f = io.open(config_home .. "/yggdrasil/input.conf", "r")
  if not f then return end
  local function num(v, lo, hi)
    local n = tonumber(v)
    if n == nil or n ~= n then return nil end
    return math.max(lo, math.min(hi, n))
  end
  local function yes(v)
    if v == "1" or v == "yes" or v == "true" or v == "on" then return true end
    if v == "0" or v == "no" or v == "false" or v == "off" then return false end
    return nil
  end
  local pointer = {
    sensitivity = function(v) return num(v, -1, 1) end,
    scroll_factor = function(v) return num(v, 0.1, 5) end,
    accel_profile = function(v) if v == "flat" or v == "adaptive" then return v end end,
    natural_scroll = yes,
    left_handed = yes,
  }
  local keyboard = {
    repeat_rate = function(v) local n = num(v, 1, 100) return n and math.floor(n) end,
    repeat_delay = function(v) local n = num(v, 100, 2000) return n and math.floor(n) end,
  }
  -- section: nil while reading every device's lines, a mouse's table in its [name] section, false in a refused one
  local all, mice, section = {}, {}, nil
  for raw in f:lines() do
    local line = raw:gsub("#.*", "")
    if line:match("^%s*%[") then
      local name = line:match("^%s*%[%s*([%l%d%-_%.:%+%(%)]+)%s*%]%s*$")
      section = name and { name = name, set = {} } or false
      if section then table.insert(mice, section) end
    else
      local k, v = line:match("^%s*([%w_]+)%s*=%s*(%S+)%s*$")
      local read = k and (pointer[k] or (section == nil and keyboard[k]) or nil)
      local value = nil
      if read then value = read(v) end
      if value ~= nil and section ~= false then (section and section.set or all)[k] = value end
    end
  end
  f:close()
  if next(all) then hl.config({ input = all }) end
  for _, m in ipairs(mice) do
    if next(m.set) then
      for n = 0, 9 do
        local spec = { name = n == 0 and m.name or (m.name .. "-" .. n) }
        for k, v in pairs(m.set) do spec[k] = v end
        hl.device(spec)
      end
    end
  end
end
do
  local ok, err = pcall(input_settings)
  if not ok then io.stderr:write(("yggdrasil: input.conf: %s\n"):format(err)) end
end

-- Windows float, centred, at the size the program asks for...
hl.window_rule({ name = "windows-float", match = { class = ".*" }, float = true, center = true })

-- Steam asks at start to have its window un-maximized, and Hyprland 0.56 maximizes it instead: its maximize requests are
-- ignored (Super+Up still maximizes it)
hl.window_rule({ name = "steam-no-self-maximize", match = { class = "^(steam)$" }, suppress_event = "maximize" })

-- Steam games are marked as games (for the direct path to the monitor above)
hl.window_rule({ name = "steam-games", match = { class = "^(steam_app_.*)$" }, content = "game" })
-- ...and drawn without their alpha channel: a game that leaves junk in it showed the desktop through in patches (The Forest's
-- rocks and water). A game never means to be see-through. Checked in a nested Hyprland 0.57 on 2026-11-05 (getprop: true).
hl.window_rule({ name = "steam-games-opaque", match = { class = "^(steam_app_.*)$" }, force_rgbx = true })

-- ...except on the screens set to tile (Settings > Displays; ~/.config/yggdrasil/tiling, one screen per line). There windows sit side
-- by side with gaps, Omarchy-style. Settings reloads this when it changes.
local function tiling_screens()
  local list = {}
  local f = io.open(config_home .. "/yggdrasil/tiling", "r")
  if f then
    for line in f:lines() do
      for name in (line:gsub("#.*", "")):gmatch("[%w%-_%.]+") do table.insert(list, name) end
    end
    f:close()
  end
  return list
end
for _, screen in ipairs(tiling_screens()) do
  hl.window_rule({ name = "tile-on-" .. screen, match = { class = ".*", workspace = "m[" .. screen .. "]" }, tile = true })
end

-- Where a window goes so that it keeps clear of the bar and the dock and opens no bigger than its screen (keep-clear.lua, with
-- plain-Lua tests in packages/session/tests/keep-clear.lua); used by the window places below and the keep-clear block after them.
local keep_clear_ok, keep_clear = pcall(dofile, "/usr/share/yggdrasil/hyprland/keep-clear.lua")
if not keep_clear_ok then
  io.stderr:write("yggdrasil: keep-clear.lua: " .. tostring(keep_clear) .. "\n")
  keep_clear = nil
end
-- the room kept free around a window made smaller to fit its screen: the session's outer gap (8 unless yours says otherwise)
local function open_gap()
  local ok, v = pcall(hl.get_config, "general.gaps_out")
  if not ok then return 8 end
  if type(v) == "table" then v = v.top or v[1] end
  if type(v) == "string" then v = v:match("%d+") end
  return tonumber(v) or 8
end

-- Windows come back where they were: when a program's last window closes, its size and place are kept (one line per program in
-- ~/.local/state/yggdrasil/window-places), and its next window opens there, if that spot is still on a screen, and no bigger
-- than that screen is now (the scale may have changed since). Dialogs and second windows of a program that already has one
-- open are left where the program puts them: a window of the same program is one of the same class or of the same process.
-- VS Code's dialogs are "code" and its main window "com.microsoft.VSCode", so its dialogs were kept and opened at the size of
-- the dialog before (2026-11-13).
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
  local pid = tonumber(w.pid) or 0
  for _, o in ipairs(hl.get_windows()) do
    if o.address ~= w.address and (o.class == w.class or (pid > 0 and tonumber(o.pid) == pid)) then n = n + 1 end
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
    local m = hl.get_monitor_at(p[1] + p[3] / 2, p[2] + p[4] / 2)
    if not m then return end
    local x, y, width, height = p[1], p[2], p[3], p[4]
    if keep_clear then
      local nx, ny, nw, nh = keep_clear.open(x, y, width, height, m, open_gap())
      if nx then x, y, width, height = nx, ny, nw, nh end
    end
    local target = "address:" .. w.address
    -- resizing keeps a window centred, so the size first, then the place
    hl.dispatch(hl.dsp.window.resize({ x = width, y = height, window = target }))
    hl.dispatch(hl.dsp.window.move({ x = x, y = y, window = target }))
  end)
  if not ok then io.stderr:write("yggdrasil: putting a window back: " .. tostring(err) .. "\n") end
end)

-- Windows keep clear of the bar and the dock (keep-clear.lua works out where a window goes). Checked when a window opens,
-- when it comes back from maximize or full screen, and when the bar or the dock appears on a screen (the dock follows the
-- main screen). On 2026-11-08 VS Code came back from maximize at 1210x806, its size from before, under the iPad screen's dock.
-- A window that opens bigger than the free room on its screen (any program's, dialogs and pop-ups too) is made to fit it and
-- centred there; one that fits keeps its size.
-- Left alone: maximized, full-screen, tiled and pinned windows, games (content type "game"), the special workspaces, and a
-- window dragged under the dock by hand.
-- back: the window has just come back from maximize or full screen (then a size as big as the free room becomes a normal one);
-- opening: it has just opened (then one bigger than its screen is made to fit and centred)
local function clear_of_bars(w, back, opening)
  if not keep_clear or not w or not w.mapped or not w.floating or (w.fullscreen or 0) ~= 0 or w.pinned then return end
  if w.content_type == "game" then return end
  local ws = w.workspace
  if not ws or (ws.name and ws.name:sub(1, 8) == "special:") then return end
  local x, y = xy(w.at)
  local width, height = xy(w.size)
  if not x or not width or width < 1 or height < 1 then return end
  local m = hl.get_monitor_at(x + width / 2, y + height / 2) or w.monitor
  if not m then return end
  local nx, ny, nw, nh
  if back then nx, ny, nw, nh = keep_clear.normal(x, y, width, height, m) end
  if not nx and opening then nx, ny, nw, nh = keep_clear.open(x, y, width, height, m, open_gap()) end
  if not nx then nx, ny, nw, nh = keep_clear.fit(x, y, width, height, m) end
  if not nx then return end
  local target = "address:" .. w.address
  -- resizing keeps a window centred, so the size first, then the place
  if nw ~= width or nh ~= height then hl.dispatch(hl.dsp.window.resize({ x = nw, y = nh, window = target })) end
  hl.dispatch(hl.dsp.window.move({ x = nx, y = ny, window = target }))
end
-- a moment later: Hyprland (and the program) settle the window's size first; address nil = every window
local clear_timers = {}
local function clear_later(address, ms, back, opening)
  local t
  t = hl.timer(function()
    clear_timers[t] = nil
    local ok, err = pcall(function()
      for _, o in ipairs(hl.get_windows()) do
        if address == nil or o.address == address then clear_of_bars(o, back, opening) end
      end
    end)
    if not ok then io.stderr:write("yggdrasil: keeping windows clear of the bar and dock: " .. tostring(err) .. "\n") end
  end, { timeout = ms, type = "oneshot" })
  clear_timers[t] = true
end
hl.on("window.open", function(w)
  if w and w.address then
    clear_later(w.address, 150, false, true)
    -- some programs (Electron) set their saved size a little after they open
    clear_later(w.address, 1200, false, true)
  end
end)
-- the window comes as a userdata object (not a table)
hl.on("window.fullscreen", function(w)
  local ok, a = pcall(function() return w.address end)
  if not ok or not a then
    local active = hl.get_active_window()
    a = active and active.address
  end
  if a then clear_later(a, 250, true) end
end)
hl.on("layer.opened", function(l)
  if l and (l.namespace == "yggdrasil-dock" or l.namespace == "yggdrasil-panel") then clear_later(nil, 300) end
end)

-- A panel pop-up (start menu, Fylgja, calendar, a tray menu, the voice island's card...) that had the keyboard closes: hand
-- the keyboard back to the window in front. Hyprland gives it back when a click lands on another window, but a click on the
-- window that was already active (VS Code's text box) left the keys going nowhere until you switched apps and back
-- (2026-11-07). Focusing the window that is already in front changes nothing when the keyboard is where it should be.
hl.on("layer.closed", function(l)
  local ok, err = pcall(function()
    if l and l.namespace and l.namespace:sub(1, 10) ~= "yggdrasil-" then return end
    for _, o in ipairs(hl.get_layers()) do
      -- another one wants the keyboard (menu to menu); the closing one may still be listed
      if o.mapped and (o.keyboard_interactivity or 0) > 0 and not (l and o.address == l.address) then return end
    end
    local w = hl.get_active_window()
    if w and w.mapped then hl.dispatch(hl.dsp.focus({ window = "address:" .. w.address })) end
  end)
  if not ok then io.stderr:write("yggdrasil: giving the keyboard back: " .. tostring(err) .. "\n") end
end)

-- Window zones (FancyZones-style; the panel's zones.rs): drag a window to the notch at the top of a screen and a sheet of layouts
-- opens under it; let go on a zone and the window fills that part of the screen. The panel notices the drag and the drop by
-- itself (the mouse's left button from the kernel, the window and pointer from Hyprland): no binding on the mouse button here,
-- because on Hyprland 0.56 one kept the release from ending the compositor's own window move (tried 2026-10-25: the window went on
-- following the pointer after the drop). Super+Shift+Z opens the sheet for the window in front.
hl.bind("SUPER + SHIFT + Z", hl.dsp.exec_cmd("yggdrasil-panelctl zones"), { desc = "Window zones for the window in front" })

-- Shortcuts (the same table as Yggdrasil's own compositor, compositor/src/shortcuts.rs).
local function cmd(keys, command, opts) hl.bind(keys, hl.dsp.exec_cmd(command), opts) end
local function dsp(keys, dispatcher, opts) hl.bind(keys, dispatcher, opts) end

cmd("SUPER + SUPER_L", "yggdrasil-panelctl start", { release = true, desc = "Start menu" })
-- a tap starts a hands-free conversation, a tap ends it (only the press is bound: which key is let go first no longer matters)
cmd("SUPER + S", "yggdrasil-panelctl ask-press", { desc = "Talk to the assistant (tap to talk hands-free, tap again to end)" })
cmd("CTRL + ALT + S", "yggdrasil-panelctl ask-press", { desc = "Talk to the assistant (tap to talk hands-free, tap again to end)" })
cmd("SUPER + V", "yggdrasil-clipboard", { desc = "Clipboard history and snippets" })
cmd("SUPER + C", "yggdrasil-panelctl calendar", { desc = "Calendar" })
cmd("SUPER + N", "yggdrasil-panelctl notifications", { desc = "Notifications" })
cmd("SUPER + E", "minos", { desc = "Files (Minos)" })
cmd("SUPER + I", "yggdrasil-settings", { desc = "Settings" })
cmd("SUPER + RETURN", "ghostty", { desc = "Terminal" })
cmd("SUPER + L", "loginctl lock-session", { desc = "Lock the screen" })
cmd("CTRL + SHIFT + ESCAPE", "yggdrasil-taskmanager", { desc = "Task Manager (again: brings it to the front)" })

cmd("ALT + F4", "yggdrasil-close", { desc = "Close the window (programs that keep running: hide it)" })
cmd("SUPER + Q", "yggdrasil-close", { desc = "Close the window (programs that keep running: hide it)" })
dsp("SUPER + F", hl.dsp.window.fullscreen({ mode = "fullscreen" }), { desc = "Full screen" })
cmd("SUPER + UP", "yggdrasil-snap up", { desc = "Maximize (again: restore), or focus up on a tiling screen" })
cmd("SUPER + DOWN", "yggdrasil-snap down", { desc = "Minimize, or focus down on a tiling screen" })
cmd("SUPER + W", "yggdrasil-close", { desc = "Close the window (as in Omarchy)" })
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
  -- the number row by name ("code:10" is not understood here and left the shortcut without a key)
  local key = tostring(n)
  dsp("SUPER + CTRL + " .. key, hl.dsp.focus({ workspace = tostring(n) }), { desc = "Desktop " .. n })
  dsp("SUPER + CTRL + SHIFT + " .. key, hl.dsp.window.move({ workspace = tostring(n) }), { desc = "Move the window to desktop " .. n })
end

-- The mouse: Super and drag to move a window, Super and right-drag to resize it.
dsp("SUPER + mouse:272", hl.dsp.window.drag(), { mouse = true, desc = "Move the window" })
dsp("SUPER + mouse:273", hl.dsp.window.resize(), { mouse = true, desc = "Resize the window" })

-- Screenshots (yggdrasil-shot): the screens are captured at once, open menus included, and you choose on that frozen picture: drag an
-- area, click a window, or Enter for the whole screen. To the clipboard and to Pictures/Screenshots.
cmd("PRINT", "yggdrasil-shot", { desc = "Screenshot (area, window or screen)" })
cmd("SUPER + SHIFT + S", "yggdrasil-shot", { desc = "Screenshot (area, window or screen)" })

-- Color Picker (PowerToys' key, packages/colorpicker): the screen freezes with a loupe; a click copies the colour in the format chosen
-- in Settings > Keyboard Shortcuts > Color Picker.
cmd("SUPER + SHIFT + C", "yggdrasil-colorpicker", { desc = "Color picker" })

-- Keyboard shortcuts and Find my mouse (the panel: yggdrasil-panelctl). Win + / shows every shortcut on a card. Find my mouse: each
-- Ctrl tapped on its own tells the panel (a release bind on a modifier), which puts a spotlight on the pointer after two taps in a
-- short time (Settings > Keyboard Shortcuts can switch it off). The binds do not consume the key: Ctrl reaches programs as always.
cmd("SUPER + SLASH", "yggdrasil-panelctl shortcuts", { desc = "Keyboard shortcuts" })
-- accessibility (docs/ACCESSIBILITY.md): the magnifier (Hyprland's zoom around the pointer) and the screen reader
cmd("SUPER + ALT + EQUAL", "yggdrasil-zoom in", { repeating = true, desc = "Magnifier: zoom in around the pointer" })
cmd("SUPER + ALT + MINUS", "yggdrasil-zoom out", { repeating = true, desc = "Magnifier: zoom out" })
cmd("SUPER + ALT + 0", "yggdrasil-zoom off", { desc = "Magnifier off" })
cmd("SUPER + ALT + S", "yggdrasil-screen-reader", { desc = "Screen reader (Orca) on or off" })
cmd("CTRL + CONTROL_L", "yggdrasil-panelctl cursor-tap", { release = true, non_consuming = true, desc = "Find my mouse (press Ctrl twice)" })
cmd("CTRL + CONTROL_R", "yggdrasil-panelctl cursor-tap", { release = true, non_consuming = true, desc = "Find my mouse (press Ctrl twice)" })

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
  -- first of all the screen between signing in and the desktop (Settings > Appearance > Boot screen), so the start-up is never
  -- a black screen; it fades away by itself once the top bar is up
  hl.exec_cmd("sh -c 'yggdrasil-splash 2>&1 | systemd-cat -t yggdrasil-splash'")
  -- programs' own minimize buttons (Hyprland ignores the request; the plugin moves the window to special:minimized)
  hl.exec_cmd("sh -c 'yggdrasil-hypr-plugin 2>&1 | systemd-cat -t yggdrasil-hypr-plugin'")
  -- a copy of this session's log that survives a crash (Hyprland deletes the folder of a finished session at the next start)
  hl.exec_cmd("sh -c 'yggdrasil-hypr-logkeep 2>&1 | systemd-cat -t yggdrasil-hypr-logkeep'")
  -- each step on its own (one failing must not stop the next), each written to the system log under "yggdrasil-session"
  hl.exec_cmd("sh -c 'systemctl --user import-environment WAYLAND_DISPLAY DISPLAY XDG_CURRENT_DESKTOP XDG_SESSION_DESKTOP XDG_SESSION_TYPE HYPRLAND_INSTANCE_SIGNATURE QT_QPA_PLATFORM QT_WAYLAND_DISABLED_INTERFACES QT_QPA_PLATFORMTHEME MANGOHUD_CONFIGFILE GDK_BACKEND MOZ_ENABLE_WAYLAND ELECTRON_OZONE_PLATFORM_HINT LIBVA_DRIVER_NAME __GLX_VENDOR_LIBRARY_NAME NVD_BACKEND 2>&1 | systemd-cat -t yggdrasil-session; dbus-update-activation-environment --systemd --all 2>&1 | systemd-cat -t yggdrasil-session; systemctl --user start yggdrasil-session.target 2>&1 | systemd-cat -t yggdrasil-session; echo autostart started for $XDG_CURRENT_DESKTOP on $WAYLAND_DISPLAY | systemd-cat -t yggdrasil-session'")
end)

-- Yours last.
run_with_monitors(config_home .. "/yggdrasil/hyprland.lua", "your hyprland.lua")

-- Each screen's own rule (Monitors, at the top), now that monitors.lua and your file have named theirs.
do
  local function is_panel(name) return name:match("^eDP%-%d+$") ~= nil or name:match("^LVDS%-%d+$") ~= nil end
  local function is_named(m)
    if named_screens[""] or named_screens[m.name] then return true end
    local desc = type(m.description) == "string" and m.description or ""
    for sel in pairs(named_screens) do
      local start = sel:match("^desc:%s*(.-)%s*$")
      if start and start ~= "" and desc:sub(1, #start) == start then return true end
    end
    return false
  end
  -- the screen's own size at the fastest rate it has for it; "preferred" when that is its own mode already (or it lists
  -- none). Counted with #, never ipairs: a list is a plain table.
  local function fastest_own(modes)
    if type(modes) ~= "table" then return "preferred" end
    local own
    for i = 1, #modes do
      if modes[i].preferred then own = modes[i] break end
    end
    own = own or modes[1]
    if not own then return "preferred" end
    local best = own
    for i = 1, #modes do
      local m = modes[i]
      if m.width == own.width and m.height == own.height and (tonumber(m.refresh_rate) or 0) > (tonumber(best.refresh_rate) or 0) then
        best = m
      end
    end
    if best == own then return "preferred" end
    return string.format("%dx%d@%.2f", best.width, best.height, best.refresh_rate)
  end
  -- what each connector was given here ("mode scale"); one with nothing has the rule for every screen, "preferred 1". A rule is
  -- added only when it changes that, so a screen already right is not set again (setting a mode can blank it for a moment).
  local given = {}
  local function give(m)
    local name = tostring(m.name)
    if is_named({ name = name, description = m.description }) then return end
    local mode, scale = fastest_own(m.available_modes), is_panel(name) and "auto" or 1
    local key = mode .. " " .. tostring(scale)
    if key == (given[name] or "preferred 1") then return end
    given[name] = key
    hl.monitor({ output = name, mode = mode, position = "auto", scale = scale })
  end
  local screens = hl.get_monitors({ all = true }) or {}
  for i = 1, #screens do give(screens[i]) end
  -- the usual laptop names too, so the first start (before Hyprland lists its screens) has the automatic scale for them; a rule
  -- for a screen that is not there does nothing
  for _, name in ipairs({ "eDP-1", "eDP-2", "LVDS-1" }) do
    if not given[name] then give({ name = name }) end
  end
  -- a screen Hyprland adds (at start, plugged in, switched on): it is at its own mode by now, this raises the rate
  hl.on("monitor.added", function(m)
    local ok, err = pcall(give, m)
    if not ok then io.stderr:write("yggdrasil: a new screen's mode: " .. tostring(err) .. "\n") end
  end)
end
