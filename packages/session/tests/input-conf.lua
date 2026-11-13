-- lua packages/session/tests/input-conf.lua: the session (yggdrasil.lua) run against a stand-in for Hyprland's `hl`, with an
-- input.conf of the test's own (Settings > Peripherals' file, in a temporary config folder): what Hyprland is given. Plain Lua,
-- no Hyprland; nothing outside the temporary folder is written.
local here = (arg and arg[0] or ""):match("^(.*)/[^/]*$") or "."

local failed = 0
local function check(name, ok, detail)
  if not ok then
    failed = failed + 1
    io.stderr:write("FAIL " .. name .. (detail and (": " .. detail) or "") .. "\n")
  end
end

-- a stand-in `hl`: every function and table Hyprland's API has, doing nothing, except that hl.config's input tables and
-- hl.device's specs are kept
local inputs, devices = {}, {}
local function stub()
  return setmetatable({}, { __index = function(t, k) local s = stub() rawset(t, k, s) return s end, __call = function() return stub() end })
end
hl = stub()
hl.config = function(c) if c.input then table.insert(inputs, c.input) end end
hl.device = function(spec) local copy = {} for k, v in pairs(spec) do copy[k] = v end table.insert(devices, copy) end
-- the devices and monitors Hyprland lists (each run sets its own; the stub's own tables would never end under ipairs)
local fake_devices, fake_monitors = {}, {}
hl.get_windows = function() return {} end
hl.get_devices = function() return fake_devices end
hl.get_monitors = function() return fake_monitors end

local function run(conf_text, devs, mons)
  inputs, devices = {}, {}
  fake_devices, fake_monitors = devs or {}, mons or {}
  local dir = os.tmpname()
  os.remove(dir)
  assert(os.execute("mkdir -p '" .. dir .. "/yggdrasil'"))
  if conf_text then
    local f = assert(io.open(dir .. "/yggdrasil/input.conf", "w"))
    f:write(conf_text)
    f:close()
  end
  local real = os.getenv
  os.getenv = function(k) if k == "XDG_CONFIG_HOME" then return dir end return real(k) end
  local ok, err = pcall(dofile, here .. "/../hyprland/yggdrasil.lua")
  os.getenv = real
  os.execute("rm -rf '" .. dir .. "'")
  check("the session file runs", ok, tostring(err))
end

-- the session's own input settings come first (keyboard layout, click to focus); the file's after them, so it wins
local function from_file() return inputs[2] or {} end

run(nil)
check("no file: only the session's own input settings", #inputs == 1 and #devices == 0)

run([[
# Written by Settings > Peripherals
sensitivity = -0.25
accel_profile = flat
scroll_factor = 9
natural_scroll = 1
left_handed = maybe
repeat_rate = 30
repeat_delay = 400

[example-gaming-mouse]
sensitivity = 0.5
left_handed = yes
repeat_rate = 5

[Not A Plain Name"]
sensitivity = 1

[example-empty]
]])
local i = from_file()
check("speed", i.sensitivity == -0.25, tostring(i.sensitivity))
check("acceleration", i.accel_profile == "flat")
check("scroll speed clamped", i.scroll_factor == 5, tostring(i.scroll_factor))
check("natural scrolling", i.natural_scroll == true)
check("a value not understood is left out", i.left_handed == nil)
check("key repeat", i.repeat_rate == 30 and i.repeat_delay == 400)
check("a refused section's lines are skipped (not every mouse's)", i.sensitivity ~= 1)
check("one mouse: itself and its nine other parts", #devices == 10, tostring(#devices))
local d = devices[1] or {}
check("the mouse's own settings", d.name == "example-gaming-mouse" and d.sensitivity == 0.5 and d.left_handed == true)
check("key repeat is for every keyboard, not in a mouse's section", d.repeat_rate == nil)
check("its parts", (devices[3] or {}).name == "example-gaming-mouse-2" and (devices[10] or {}).name == "example-gaming-mouse-9")

-- the pen, touchscreen and TrackPoint rules, by the devices a computer has
local function spec_for(name)
  for _, d in ipairs(devices) do if d.name == name then return d end end
end

-- a desktop: a wide monitor and a streamed iPad screen (a headless output), a gaming mouse with its extra parts, a keyboard and
-- the stream's own virtual mouse, touchscreen and pen. No laptop panel: nothing is mapped and no mouse scrolls on a button.
run(nil, {
  { name = "example-gaming-mouse", type = "pointer" },
  { name = "example-gaming-mouse-1", type = "pointer" },
  { name = "example-gaming-keyboard", type = "keyboard" },
  { name = "mouse-passthrough", type = "pointer" },
  { name = "touch-passthrough", type = "touch" },
  { name = "pen-passthrough", type = "tablet" },
}, { { name = "DP-1" }, { name = "HEADLESS-2" } })
check("a desktop: no device is given a screen or a scroll button", #devices == 0, tostring(#devices))

-- a laptop (ThinkPad style): its panel, a TrackPoint, a touchpad with its "-mouse" companion, the screen's pen and touchscreen,
-- an external monitor and a drawing tablet on a cable
run(nil, {
  { name = "tpps/2-elan-trackpoint", type = "pointer" },
  { name = "elan0001:00-04f3:3200-touchpad", type = "pointer" },
  { name = "elan0001:00-04f3:3200-mouse", type = "pointer" },
  { name = "wacom-hid-5349-pen", type = "tablet" },
  { name = "wacom-hid-5349-finger", type = "touch" },
  { name = "wacom-intuos-pro-m-pen", type = "tablet" },
  { name = "example-usb-mouse", type = "pointer" },
}, { { name = "eDP-1" }, { name = "DP-3" } })
check("the screen's pen draws on the panel", (spec_for("wacom-hid-5349-pen") or {}).output == "eDP-1")
check("the touchscreen is the panel's", (spec_for("wacom-hid-5349-finger") or {}).output == "eDP-1")
check("a drawing tablet on a cable is left alone", spec_for("wacom-intuos-pro-m-pen") == nil)
local tp = spec_for("tpps/2-elan-trackpoint") or {}
check("the TrackPoint scrolls on its middle button", tp.scroll_method == "on_button_down" and tp.scroll_button == 274)
check("the touchpad's companion mouse too", (spec_for("elan0001:00-04f3:3200-mouse") or {}).scroll_button == 274)
check("the touchpad itself and other mice are left alone",
  spec_for("elan0001:00-04f3:3200-touchpad") == nil and spec_for("example-usb-mouse") == nil)

if failed > 0 then
  io.stderr:write(failed .. " failed\n")
  os.exit(1)
end
print("input-conf: all passed")
