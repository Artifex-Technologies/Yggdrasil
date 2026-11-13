-- lua packages/session/tests/color-filter.lua: Settings > Accessibility's colour filter reaches Hyprland. The session (yggdrasil.lua)
-- runs against a stand-in for Hyprland's `hl` with a temporary config folder: `color_filter` in accessibility.conf names the shader
-- Settings wrote (~/.config/yggdrasil/shaders/NAME.frag), which becomes decoration.screen_shader; none, a missing file or an odd
-- name gives no shader. Plain Lua, no Hyprland.
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

-- the screen_shader the session gave Hyprland (nil when none), with `filter` in accessibility.conf and `shader` written or not
local function run(filter, shader)
  hl = stub()
  local shown
  hl.config = function(c) if c.decoration and c.decoration.screen_shader ~= nil then shown = c.decoration.screen_shader end end
  hl.get_windows = function() return {} end
  hl.get_devices = function() return {} end
  hl.get_monitors = function() return {} end
  local dir = os.tmpname()
  os.remove(dir)
  assert(os.execute("mkdir -p '" .. dir .. "/yggdrasil/shaders'"))
  if filter then
    local f = assert(io.open(dir .. "/yggdrasil/accessibility.conf", "w"))
    f:write("# Settings > Accessibility\ncolor_filter = " .. filter .. "\n")
    f:close()
  end
  if shader then
    local f = assert(io.open(dir .. "/yggdrasil/shaders/" .. shader .. ".frag", "w"))
    f:write("#version 300 es\n")
    f:close()
  end
  local real = os.getenv
  os.getenv = function(k) if k == "XDG_CONFIG_HOME" then return dir end return real(k) end
  local ok, err = pcall(dofile, here .. "/../hyprland/yggdrasil.lua")
  os.getenv = real
  os.execute("rm -rf '" .. dir .. "'")
  check("the session file runs", ok, tostring(err))
  return shown and shown:gsub("^.*/yggdrasil/", "")
end

check("no accessibility.conf: no shader", run(nil, nil) == nil)
check("greyscale: its shader", run("greyscale", "greyscale") == "shaders/greyscale.frag", tostring(run("greyscale", "greyscale")))
check("none: no shader", run("none", "none") == nil)
check("named, but its file is missing: no shader", run("tritanopia", nil) == nil)
check("not a plain name: no shader", run("../x", "x") == nil)

if failed > 0 then
  io.stderr:write(failed .. " failed\n")
  os.exit(1)
end
print("color-filter: all passed")
