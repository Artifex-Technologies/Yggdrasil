-- lua packages/session/tests/monitor-modes.lua: the session (yggdrasil.lua) run against a stand-in for Hyprland's `hl` with the
-- mode lists of two real monitors (an X34 and an LG DualUp, as `hyprctl monitors all` gave them on 2026-11-13): a screen nothing
-- names comes up at its own resolution at the fastest rate it has for it, at start, on reload and when it is plugged in later;
-- a screen monitors.lua or your hyprland.lua names keeps what they say. Plain Lua, no Hyprland; config files in a temporary
-- folder of the test's own.
--
-- The stand-in is strict: it has every function yggdrasil.lua calls, and anything else it is asked for is an error (a stand-in
-- that makes up whatever it is asked for once made a test loop for ever and eat 43 GB).
local here = (arg and arg[0] or ""):match("^(.*)/[^/]*$") or "."

local failed = 0
local function check(name, ok, detail)
  if not ok then
    failed = failed + 1
    io.stderr:write("FAIL " .. name .. (detail and (": " .. detail) or "") .. "\n")
  end
end

local function strict(name, t)
  return setmetatable(t, { __index = function(_, k) error("the stand-in has no " .. name .. "." .. tostring(k), 2) end })
end

local rules, handlers, screens = {}, {}, {}
local function action(what) return function(a) return { what, a } end end
hl = strict("hl", {
  monitor = function(spec) local copy = {} for k, v in pairs(spec) do copy[k] = v end table.insert(rules, copy) end,
  config = function() end,
  device = function() end,
  bind = function() end,
  window_rule = function() end,
  dispatch = function() end,
  exec_cmd = function() end,
  timer = function() return {} end,
  on = function(event, fn) handlers[event] = handlers[event] or {} table.insert(handlers[event], fn) end,
  get_monitors = function() return screens end,
  get_monitor_at = function() return nil end,
  get_devices = function() return {} end,
  get_windows = function() return {} end,
  get_layers = function() return {} end,
  get_active_window = function() return nil end,
  get_config = function() return nil end,
  dsp = strict("hl.dsp", {
    exec_cmd = action("exec"),
    focus = action("focus"),
    window = strict("hl.dsp.window", {
      bring_to_top = action("bring_to_top"), cycle_next = action("cycle_next"), drag = action("drag"), float = action("float"),
      fullscreen = action("fullscreen"), move = action("move"), resize = action("resize"),
    }),
    workspace = strict("hl.dsp.workspace", { toggle_special = action("toggle_special") }),
  }),
})

-- "WxH@rate" lines into Hyprland's available_modes; the first is the monitor's own (preferred) mode, as Hyprland lists them
local function modes(list)
  local out = {}
  for i, s in ipairs(list) do
    local w, h, r = s:match("^(%d+)x(%d+)@([%d.]+)$")
    out[i] = { width = tonumber(w), height = tonumber(h), refresh_rate = tonumber(r), preferred = i == 1 }
  end
  return out
end
local function x34(name)
  return { name = name or "DP-1", description = "Acer Technologies X34 0000", available_modes = modes({
    "3440x1440@60.00", "3840x2160@59.94", "3840x2160@29.97", "2560x1440@59.95", "1920x1080@60.00",
    "3440x1440@180.00", "3440x1440@143.85", "3440x1440@100.00", "3440x1440@85.00", "3440x1440@75.00",
  }) }
end
local function dualup(name)
  return { name = name or "DP-3", description = "LG Electronics LG SDQHD 0000", available_modes = modes({
    "2560x2880@59.98", "1920x1080@60.00", "1920x1080@59.94", "1920x1080@50.00", "1920x1080@29.97", "1280x720@59.94",
    "1280x720@50.00", "1024x768@60.00", "800x600@60.32", "720x576@50.00", "720x480@59.94",
  }) }
end

-- yggdrasil.lua loaded with these screens already there (a reload) and these files in its config folder
local function load(present, files)
  rules, handlers, screens = {}, {}, present
  local dir = os.tmpname()
  os.remove(dir)
  assert(os.execute("mkdir -p '" .. dir .. "/yggdrasil'"))
  for name, text in pairs(files or {}) do
    local f = assert(io.open(dir .. "/yggdrasil/" .. name, "w"))
    f:write(text)
    f:close()
  end
  local real = os.getenv
  os.getenv = function(k) if k == "XDG_CONFIG_HOME" then return dir end return real(k) end
  local ok, err = pcall(dofile, here .. "/../hyprland/yggdrasil.lua")
  os.getenv = real
  os.execute("rm -rf '" .. dir .. "'")
  check("the session file runs against the strict stand-in", ok, tostring(err))
  check("the session listens for screens being added", handlers["monitor.added"] and #handlers["monitor.added"] == 1)
end
-- Hyprland adding a screen (at start, plugged in, switched on)
local function add(m)
  table.insert(screens, m)
  for _, fn in ipairs(handlers["monitor.added"] or {}) do fn(m) end
end
local function for_output(name)
  local out = {}
  for _, r in ipairs(rules) do if r.output == name then table.insert(out, r) end end
  return out
end
local function one(name, mode, scale)
  local r = for_output(name)
  return #r == 1 and r[1].mode == mode and r[1].scale == scale and r[1].position == "auto"
end
local function show(name)
  local t = {}
  for _, r in ipairs(for_output(name)) do table.insert(t, tostring(r.mode) .. " " .. tostring(r.scale)) end
  return name .. ": [" .. table.concat(t, ", ") .. "]"
end

-- the first start: no screens yet, then Hyprland adds them
load({})
check("every screen: its own mode until it is known", one("", "preferred", 1), show(""))
check("laptop names before any screen is listed: automatic scale", one("eDP-1", "preferred", "auto") and one("LVDS-1", "preferred", "auto"))
add(x34())
check("the X34 at its own size, fastest (not 60 Hz, not 3840x2160)", one("DP-1", "3440x1440@180.00", 1), show("DP-1"))
add(dualup())
check("the DualUp at 2560x2880@59.98 (not 800x600@60.32): its own mode, no rule of its own", #for_output("DP-3") == 0, show("DP-3"))

-- a reload with both there: the same, worked out before any event
load({ x34(), dualup() })
check("reload: the X34 keeps 180 Hz", one("DP-1", "3440x1440@180.00", 1), show("DP-1"))
check("reload: the DualUp keeps its own mode", #for_output("DP-3") == 0, show("DP-3"))
add(x34())
check("the X34 again (switched off and on): no second rule", #for_output("DP-1") == 1, show("DP-1"))

-- the X34 on its own
load({})
add(x34("HDMI-A-1"))
check("the X34 alone, on another connector", one("HDMI-A-1", "3440x1440@180.00", 1), show("HDMI-A-1"))

-- another monitor plugged into the connector the X34 had: its own mode back
load({ x34() })
add(dualup("DP-1"))
local r = for_output("DP-1")
check("a DualUp where the X34 was: back to preferred", #r == 2 and r[2].mode == "preferred" and r[2].scale == 1, show("DP-1"))

-- Settings > Displays' lines win: nothing is worked out for a screen monitors.lua names (by connector or by description)
load({ x34(), dualup() }, { ["monitors.lua"] =
  'hl.monitor({ output = "DP-1", mode = "3440x1440@100.00", position = "0x0", scale = 1 })\n' })
check("monitors.lua's DP-1 line is the only one", #for_output("DP-1") == 1 and for_output("DP-1")[1].mode == "3440x1440@100.00", show("DP-1"))
add(x34())
check("and stays the only one when the X34 is added again", #for_output("DP-1") == 1, show("DP-1"))
load({}, { ["monitors.lua"] = 'hl.monitor({ output = "desc:Acer Technologies X34", mode = "3440x1440@143.85", position = "0x0", scale = 1 })\n' })
add(x34())
check("a monitor named by its description: nothing worked out", #for_output("DP-1") == 0, show("DP-1"))

-- your own hyprland.lua: a screen it names, and a rule of yours for every screen
load({}, { ["hyprland.lua"] = 'hl.monitor({ output = "DP-1", mode = "3440x1440@85", position = "0x0", scale = 1 })\n' })
add(x34())
check("a screen your hyprland.lua names keeps your line", #for_output("DP-1") == 1 and for_output("DP-1")[1].mode == "3440x1440@85", show("DP-1"))
load({}, { ["hyprland.lua"] = 'hl.monitor({ output = "", mode = "highres", position = "auto", scale = 1 })\n' })
add(x34())
check("a rule of yours for every screen: no screen gets one of its own", #for_output("DP-1") == 0 and #for_output("eDP-1") == 0, show("DP-1"))

-- a laptop: its panel's own size at its fastest rate, with the automatic scale
local panel = { name = "eDP-1", description = "Some Panel", available_modes = modes({ "2880x1800@60.00", "2880x1800@120.00", "1920x1200@120.50" }) }
load({ panel })
check("the laptop's panel: own size, fastest, automatic scale", one("eDP-1", "2880x1800@120.00", "auto"), show("eDP-1"))
add(panel)
check("the panel added again: no second rule", #for_output("eDP-1") == 1, show("eDP-1"))
load({})
add({ name = "eDP-1", available_modes = modes({ "1920x1080@60.00" }) })
check("a 60 Hz panel at start: the automatic-scale rule, nothing more", one("eDP-1", "preferred", "auto"), show("eDP-1"))

-- screens Hyprland gives no modes for (a nested or headless one)
load({})
add({ name = "HEADLESS-2" })
check("no modes: no rule of its own", #for_output("HEADLESS-2") == 0)

if failed > 0 then
  io.stderr:write(failed .. " failed\n")
  os.exit(1)
end
print("monitor-modes: all passed")
