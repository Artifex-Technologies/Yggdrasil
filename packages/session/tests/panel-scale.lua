-- lua packages/session/tests/panel-scale.lua: the session (yggdrasil.lua) run against a stand-in for Hyprland's `hl`: a
-- laptop's own screen gets Hyprland's automatic scale, monitors on a cable keep scale 1, and a screen Settings > Displays named
-- in monitors.lua (a temporary config folder) keeps what it says. Plain Lua, no Hyprland.
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
local rules = {}
hl.monitor = function(spec) local copy = {} for k, v in pairs(spec) do copy[k] = v end table.insert(rules, copy) end
local fake_monitors = {}
hl.get_windows = function() return {} end
hl.get_devices = function() return {} end
hl.get_monitors = function() return fake_monitors end

local function run(monitors_lua, mons)
  rules, fake_monitors = {}, mons
  local dir = os.tmpname()
  os.remove(dir)
  assert(os.execute("mkdir -p '" .. dir .. "/yggdrasil'"))
  if monitors_lua then
    local f = assert(io.open(dir .. "/yggdrasil/monitors.lua", "w"))
    f:write(monitors_lua)
    f:close()
  end
  local real = os.getenv
  os.getenv = function(k) if k == "XDG_CONFIG_HOME" then return dir end return real(k) end
  local ok, err = pcall(dofile, here .. "/../hyprland/yggdrasil.lua")
  os.getenv = real
  os.execute("rm -rf '" .. dir .. "'")
  check("the session file runs", ok, tostring(err))
end

-- the rules given for one output, in order
local function for_output(name)
  local out = {}
  for _, r in ipairs(rules) do if r.output == name then table.insert(out, r) end end
  return out
end

run(nil, { { name = "eDP-1" }, { name = "DP-1" }, { name = "eDP-3" } })
local all = for_output("")
check("every screen: its own mode, scale 1", #all == 1 and all[1].scale == 1 and all[1].mode == "preferred")
local p = for_output("eDP-1")
check("the laptop's own screen: automatic scale", #p == 1 and p[1].scale == "auto" and p[1].mode == "preferred" and p[1].position == "auto")
check("another panel Hyprland lists", #for_output("eDP-3") == 1 and for_output("eDP-3")[1].scale == "auto")
check("the usual names before Hyprland lists any", #for_output("eDP-2") == 1 and #for_output("LVDS-1") == 1)
check("a monitor on a cable: no rule of its own", #for_output("DP-1") == 0)

run('hl.monitor({ output = "eDP-1", mode = "2880x1800@60.00", position = "0x0", scale = 1.5 })\n'
  .. 'hl.monitor({ output = "DP-1", mode = "3440x1440@100.00", position = "1646x0", scale = 1 })\n', { { name = "eDP-1" }, { name = "DP-1" } })
p = for_output("eDP-1")
check("named in monitors.lua: only what Settings wrote", #p == 1 and p[1].scale == 1.5, tostring(#p))
local dp = for_output("DP-1")
check("the external monitor exactly as written", #dp == 1 and dp[1].scale == 1 and dp[1].mode == "3440x1440@100.00")
check("panels it does not name still get the automatic scale", #for_output("eDP-2") == 1 and for_output("eDP-2")[1].scale == "auto")

run(nil, { { name = "HDMI-A-1" }, { name = "DP-2" } })
local count = 0
for _, r in ipairs(rules) do if r.scale == "auto" then count = count + 1 end end
check("a desktop: the automatic rules name only laptop connectors, which it does not have", count == 3 and #for_output("HDMI-A-1") == 0)

if failed > 0 then
  io.stderr:write(failed .. " failed\n")
  os.exit(1)
end
print("panel-scale: all passed")
