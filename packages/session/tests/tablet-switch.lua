-- lua packages/session/tests/tablet-switch.lua: the session (yggdrasil.lua) run against a stand-in for Hyprland's `hl`: a
-- 2-in-1's tablet-mode switch is bound to /usr/lib/yggdrasil/tablet, the lid is not, and nothing is bound when the program is
-- not installed. Plain Lua, no Hyprland; the program's presence is pretended (io.open), nothing is run.
local here = (arg and arg[0] or ""):match("^(.*)/[^/]*$") or "."

local failed = 0
local function check(name, ok, detail)
  if not ok then
    failed = failed + 1
    io.stderr:write("FAIL " .. name .. (detail and (": " .. detail) or "") .. "\n")
  end
end

local function stub()
  return setmetatable({}, { __index = function(t, k) local s = stub() rawset(t, k, s) return s end, __call = function() return stub() end })
end
hl = stub()
local binds = {}
hl.bind = function(keys, dispatcher, opts) table.insert(binds, { keys = keys, cmd = dispatcher, opts = opts or {} }) end
hl.dsp.exec_cmd = function(cmd) return cmd end
local fake_devices = {}
hl.get_windows = function() return {} end
hl.get_devices = function() return fake_devices end
hl.get_monitors = function() return {} end

local real_open = io.open
local function run(installed, devs)
  binds, fake_devices = {}, devs
  local dir = os.tmpname()
  os.remove(dir)
  local real_getenv = os.getenv
  os.getenv = function(k) if k == "XDG_CONFIG_HOME" then return dir end return real_getenv(k) end
  io.open = function(p, m)
    if p == "/usr/lib/yggdrasil/tablet" then
      if installed then return { close = function() end } end
      return nil
    end
    return real_open(p, m)
  end
  local ok, err = pcall(dofile, here .. "/../hyprland/yggdrasil.lua")
  io.open, os.getenv = real_open, real_getenv
  check("the session file runs", ok, tostring(err))
end

local function switch_binds()
  local out = {}
  for _, b in ipairs(binds) do
    if type(b.keys) == "string" and b.keys:sub(1, 7) == "switch:" then out[b.keys] = b end
  end
  return out
end

run(true, {
  { name = "lid-switch", type = "switch" },
  { name = "thinkpad-extra-buttons", type = "switch" },
  { name = "wacom-hid-5349-finger", type = "touch" },
})
local s = switch_binds()
local on, off = s["switch:on:thinkpad-extra-buttons"], s["switch:off:thinkpad-extra-buttons"]
check("the switch Hyprland lists turns tablet mode on", on and on.cmd == "/usr/lib/yggdrasil/tablet mode on")
check("and off", off and off.cmd == "/usr/lib/yggdrasil/tablet mode off")
check("also while the screen is locked", on and on.opts.locked == true)
check("the kernel's own name for it", s["switch:on:ThinkPad Extra Buttons"] ~= nil and s["switch:off:Intel Virtual Switches"] ~= nil)
check("never the lid", s["switch:on:lid-switch"] == nil)
check("never a touchscreen", s["switch:on:wacom-hid-5349-finger"] == nil)
local count = 0
for _ in pairs(s) do count = count + 1 end
check("on and off for each of six names", count == 12, tostring(count))

run(false, { { name = "thinkpad-extra-buttons", type = "switch" } })
check("the program not installed: no binds", next(switch_binds()) == nil)

if failed > 0 then
  io.stderr:write(failed .. " failed\n")
  os.exit(1)
end
print("tablet-switch: all passed")
