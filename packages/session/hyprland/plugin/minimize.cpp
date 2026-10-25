// A program's own minimize button (VS Code, Minos, Spotify, Steam...) asks the compositor to minimize its window. Hyprland
// records that request and does nothing with it, and tells nobody (no event, no Lua hook). This plugin listens for the request
// and puts the window on the hidden "minimized" workspace, where Super+Down and the dock put minimized windows in Yggdrasil's
// session; the dock brings it back from there.
//
// Built per Hyprland version by yggdrasil-hypr-plugin (Hyprland only loads plugins built against its own headers).

#include <hyprland/src/plugins/PluginAPI.hpp>
#include <hyprland/src/event/EventBus.hpp>
#include <hyprland/src/desktop/view/Window.hpp>
#include <hyprland/src/protocols/XDGShell.hpp>
#include <hyprland/src/xwayland/XSurface.hpp>
#include <hyprland/src/managers/eventLoop/EventLoopManager.hpp>
#include <hyprland/src/desktop/state/ViewState.hpp>

#include <cstdio>
#include <unordered_map>

static HANDLE                                               PHANDLE = nullptr;
static CHyprSignalListener                                  openListener;
static CHyprSignalListener                                  closeListener;
// one listener per window, for the request; dropped with the window
static std::unordered_map<Desktop::View::CWindow*, CHyprSignalListener> requestListeners;

static void minimize(PHLWINDOWREF ref) {
    // not inside the protocol's signal: Hyprland resets the request state right after it, and a move from there would run
    // in the middle of its own handling
    g_pEventLoopManager->doLater([ref] {
        const auto w = ref.lock();
        if (!w)
            return;
        char lua[160];
        std::snprintf(lua, sizeof(lua), "hl.dsp.window.move({ workspace = \"special:minimized\", follow = false, window = \"address:0x%lx\" })",
                      (unsigned long)(uintptr_t)w.get());
        HyprlandAPI::invokeHyprctlCommand("dispatch", lua);
    });
}

static void watch(PHLWINDOW w) {
    PHLWINDOWREF ref = w;
    if (const auto xdg = w->m_xdgSurface.lock(); xdg && xdg->m_toplevel) {
        const auto top = xdg->m_toplevel;
        requestListeners[w.get()] = top->m_events.stateChanged.listen([ref, top] {
            if (const auto t = top.lock(); t && t->m_state.requestsMinimize.value_or(false))
                minimize(ref);
        });
    } else if (const auto x = w->m_xwaylandSurface.lock(); x) {
        const auto xs = w->m_xwaylandSurface;
        requestListeners[w.get()] = x->m_events.stateChanged.listen([ref, xs] {
            if (const auto s = xs.lock(); s && s->m_state.requestsMinimize.value_or(false))
                minimize(ref);
        });
    }
}

APICALL EXPORT std::string PLUGIN_API_VERSION() {
    return HYPRLAND_API_VERSION;
}

APICALL EXPORT PLUGIN_DESCRIPTION_INFO PLUGIN_INIT(HANDLE handle) {
    PHANDLE = handle;
    if (std::string(__hyprland_api_get_hash()) != __hyprland_api_get_client_hash())
        throw std::runtime_error("yggdrasil-minimize was built for another Hyprland version");

    openListener = Event::bus()->m_events.window.open.listen([](PHLWINDOW w) { watch(w); });
    closeListener = Event::bus()->m_events.window.close.listen([](PHLWINDOW w) { requestListeners.erase(w.get()); });
    // windows that were already open when the plugin was loaded
    for (const auto& w : Desktop::viewState()->windows())
        if (w->m_isMapped)
            watch(w);
    return {"yggdrasil-minimize", "A program's minimize button puts its window on the hidden \"minimized\" workspace", "Yggdrasil", "1"};
}

APICALL EXPORT void PLUGIN_EXIT() {
    requestListeners.clear();
    openListener.reset();
    closeListener.reset();
}
