-- lua packages/session/tests/window-open.lua: the session (yggdrasil.lua) run against a stand-in for Hyprland's `hl` on a laptop
-- screen at scale 1.5, with a window-places file of the test's own: what happens to windows as they open (their kept places,
-- dialogs, windows bigger than the screen) and what is kept when they close. Plain Lua, no Hyprland; nothing outside the
-- temporary folder is written.
local here = (arg and arg[0] or ""):match("^(.*)/[^/]*$") or "."

local failed = 0
local function check(name, ok, detail)
  if not ok then
    failed = failed + 1
    io.stderr:write("FAIL " .. name .. (detail and (": " .. detail) or "") .. "\n")
  end
end

local function stub()
  -- number keys are nil, so ipairs over a stand-in ends at once (a stand-in for each index never ends)
  return setmetatable({}, { __index = function(t, k)
    if type(k) == "number" then return nil end
    local s = stub() rawset(t, k, s) return s
  end, __call = function() return stub() end })
end

-- the laptop's 1920x1200 panel at scale 1.5 (1280x800 to lay out), the bar 28 high, the dock 70: 1280x702 free
local laptop = { name = "eDP-1", x = 0, y = 0, width = 1920, height = 1200, scale = 1.5, transform = 0,
  reserved = { top = 28.0, right = 0.0, bottom = 70.0, left = 0.0 } }
local windows, handlers, timers, done = {}, {}, {}, {}
hl = stub()
hl.config = function() end
hl.get_devices = function() return {} end
hl.get_monitors = function() return { laptop } end
hl.get_windows = function() return windows end
hl.get_monitor_at = function(x, y)
  local s = laptop.scale
  if x >= laptop.x and y >= laptop.y and x < laptop.x + laptop.width / s and y < laptop.y + laptop.height / s then return laptop end
end
-- the outer gap as Hyprland reports it ("8 8 8 8")
hl.get_config = function(k) if k == "general.gaps_out" then return "8 8 8 8" end end
hl.on = function(event, fn) handlers[event] = handlers[event] or {} table.insert(handlers[event], fn) end
hl.timer = function(fn) table.insert(timers, fn) return {} end
hl.dispatch = function(d) table.insert(done, d) end
-- (the mouse bindings call them with nothing)
hl.dsp.window.resize = function(a) a = a or {} return { "resize", a.x, a.y, a.window } end
hl.dsp.window.move = function(a) a = a or {} return { "move", a.x, a.y, a.window } end

local function fire(event, w)
  for _, fn in ipairs(handlers[event] or {}) do fn(w) end
end
-- a window as Hyprland opens it (floating, centred on the screen) and the session's handlers on it, the timers included
local function open(w)
  w.mapped, w.floating, w.fullscreen, w.pinned = true, true, 0, false
  w.workspace = { name = "1" }
  w.monitor = laptop
  table.insert(windows, w)
  done, timers = {}, {}
  fire("window.open", w)
  local pending = timers
  timers = {}
  for _, fn in ipairs(pending) do fn() end
  return done
end
local function close(w)
  for i, o in ipairs(windows) do if o == w then table.remove(windows, i) break end end
  fire("window.close", w)
end
-- the last size and place asked for a window ("x y w h"), or nil when nothing was changed
local function last(d, address)
  local size, at
  for _, x in ipairs(d) do
    if x[4] == "address:" .. address then
      if x[1] == "resize" then size = x[2] .. "x" .. x[3] else at = x[2] .. "," .. x[3] end
    end
  end
  if not size and not at then return nil end
  return (at or "-") .. " " .. (size or "-")
end

local dir = os.tmpname()
os.remove(dir)
assert(os.execute("mkdir -p '" .. dir .. "/config/yggdrasil' '" .. dir .. "/state/yggdrasil'"))
local places = dir .. "/state/yggdrasil/window-places"
do
  -- VS Code's window kept at scale 1 (1208x806, taller than the 702 free now), and a dialog kept before the fix
  local f = assert(io.open(places, "w"))
  f:write("com.microsoft.VSCode\t100\t50\t1208\t806\ncode\t300\t100\t900\t700\n")
  f:close()
end
local real_getenv, real_dofile = os.getenv, dofile
os.getenv = function(k)
  if k == "XDG_CONFIG_HOME" then return dir .. "/config" end
  if k == "XDG_STATE_HOME" then return dir .. "/state" end
  return real_getenv(k)
end
-- the installed keep-clear.lua is this repository's
dofile = function(p)
  if p == "/usr/share/yggdrasil/hyprland/keep-clear.lua" then p = here .. "/../hyprland/keep-clear.lua" end
  return real_dofile(p)
end
local ok, err = pcall(real_dofile, here .. "/../hyprland/yggdrasil.lua")
check("the session file runs", ok, tostring(err))

if ok then
  -- VS Code's main window comes back at its kept place, but no taller than the room the screen leaves now
  local code = { address = "1", class = "com.microsoft.VSCode", pid = 100, at = { 36, 0 }, size = { 1208, 800 } }
  local d = open(code)
  check("a kept size from before the scale changed fits the screen", last(d, "1") == "36,36 1208x686", last(d, "1"))
  code.at, code.size = { 36, 36 }, { 1208, 686 }

  -- its trust dialog: another class ("code"), the same process. Not given the kept "code" place, left at its own size.
  local trust = { address = "2", class = "code", pid = 100, at = { 374, 311 }, size = { 532, 177 } }
  d = open(trust)
  check("a dialog is not put at a kept place", last(d, "2") == nil, last(d, "2"))
  close(trust)
  local f = assert(io.open(places))
  local text = f:read("a")
  f:close()
  check("a dialog's place is not kept when it closes", text:find("code\t300\t100\t900\t700", 1, true) ~= nil, text)

  -- a dialog that opens bigger than the screen: as big as the room less the gap, centred
  local huge = { address = "3", class = "code", pid = 100, at = { -110, 50 }, size = { 1500, 900 } }
  d = open(huge)
  check("a dialog bigger than the screen fits it", last(d, "3") == "8,36 1264x686", last(d, "3"))
  close(huge)

  -- a program of its own (another process) with no kept place: a normal window is left alone, a too-wide one is fitted
  local small = { address = "4", class = "org.example.small", pid = 200, at = { 340, 200 }, size = { 600, 400 } }
  d = open(small)
  check("a normal window is left alone", last(d, "4") == nil, last(d, "4"))
  local wide = { address = "5", class = "org.example.wide", pid = 300, at = { -310, 100 }, size = { 1900, 500 } }
  d = open(wide)
  check("a window wider than the screen fits it", last(d, "5") == "8,129 1264x500", last(d, "5"))

  -- a game is left alone, however big
  local game = { address = "6", class = "steam_app_1", pid = 400, at = { -320, -200 }, size = { 1920, 1200 }, content_type = "game" }
  d = open(game)
  check("a game is left alone", last(d, "6") == nil, last(d, "6"))
end

os.getenv, dofile = real_getenv, real_dofile
os.execute("rm -rf '" .. dir .. "'")
if failed > 0 then
  io.stderr:write(failed .. " failed\n")
  os.exit(1)
end
print("window-open: all passed")
