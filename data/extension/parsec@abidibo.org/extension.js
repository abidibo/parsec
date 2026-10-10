// Parsec's hands inside GNOME Shell.
//
// Exports org.abidibo.Parsec.Shell on the session bus. The launcher (a
// separate GTK process) calls it for the things a Wayland client cannot do
// on its own: list and focus windows, type Ctrl+V into whatever has focus,
// and hear about clipboard changes without polling. There is no UI and no
// logic beyond that; the daemon decides what to show and when.

import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import St from 'gi://St';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const BUS_NAME = 'org.abidibo.Parsec.Shell';
const OBJECT_PATH = '/org/abidibo/Parsec/Shell';
const VERSION = '0.1.0';

const IFACE = `
<node>
  <interface name="${BUS_NAME}">
    <property name="Version" type="s" access="read"/>
    <method name="ListWindows">
      <arg type="aa{sv}" direction="out" name="windows"/>
    </method>
    <method name="ActivateWindow">
      <arg type="t" direction="in" name="id"/>
      <arg type="b" direction="out" name="ok"/>
    </method>
    <method name="CloseWindow">
      <arg type="t" direction="in" name="id"/>
      <arg type="b" direction="out" name="ok"/>
    </method>
    <method name="Paste">
      <arg type="s" direction="in" name="text"/>
    </method>
    <method name="GetClipboard">
      <arg type="s" direction="out" name="text"/>
    </method>
    <signal name="ClipboardChanged">
      <arg type="s" name="text"/>
    </signal>
  </interface>
</node>`;

// Terminals take Ctrl+Shift+V; everything else takes Ctrl+V.
const TERMINAL_CLASSES = new Set([
    'kitty', 'alacritty', 'org.wezfurlong.wezterm', 'wezterm', 'foot',
    'footclient', 'gnome-terminal-server', 'org.gnome.terminal',
    'org.gnome.ptyxis', 'konsole', 'xfce4-terminal', 'xterm', 'urxvt',
    'tilix', 'terminator', 'com.mitchellh.ghostty', 'ghostty', 'st',
    'org.codeberg.dnkl.foot', 'wezterm-gui', 'rio', 'contour', 'tilda',
    'guake', 'org.gnome.console', 'kgx',
]);

const PARSEC_CLASS = 'org.abidibo.parsec';

class Service {
    constructor() {
        this._dbus = Gio.DBusExportedObject.wrapJSObject(IFACE, this);
        this._dbus.export(Gio.DBus.session, OBJECT_PATH);
        this._nameId = Gio.DBus.session.own_name(
            BUS_NAME,
            Gio.BusNameOwnerFlags.REPLACE,
            null,
            null);

        this._selection = global.display.get_selection();
        this._selectionId = this._selection.connect('owner-changed',
            (_sel, type, _source) => {
                if (type === Meta.SelectionType.SELECTION_CLIPBOARD)
                    this._onClipboardChanged();
            });

        this._keyboard = null;
        this._pasteTimeout = 0;
    }

    destroy() {
        if (this._pasteTimeout) {
            GLib.source_remove(this._pasteTimeout);
            this._pasteTimeout = 0;
        }
        if (this._selectionId) {
            this._selection.disconnect(this._selectionId);
            this._selectionId = 0;
        }
        if (this._nameId) {
            Gio.DBus.session.unown_name(this._nameId);
            this._nameId = 0;
        }
        this._dbus.unexport();
        this._keyboard = null;
    }

    get Version() {
        return VERSION;
    }

    // ------------------------------------------------------------ windows

    _windows() {
        return global.get_window_actors()
            .map(a => a.meta_window)
            .filter(w => w && !w.skip_taskbar &&
                w.get_window_type() === Meta.WindowType.NORMAL &&
                (w.get_wm_class() ?? '').toLowerCase() !== PARSEC_CLASS);
    }

    _find(id) {
        return this._windows().find(w => w.get_id() === id) ?? null;
    }

    ListWindows() {
        const tracker = Shell.WindowTracker.get_default();
        const focus = global.display.focus_window;
        return this._windows().map(w => {
            const app = tracker.get_window_app(w);
            const ws = w.get_workspace();
            return {
                id: GLib.Variant.new_uint64(w.get_id()),
                title: GLib.Variant.new_string(w.get_title() ?? ''),
                wm_class: GLib.Variant.new_string(w.get_wm_class() ?? ''),
                app_id: GLib.Variant.new_string(app?.get_id() ?? ''),
                app_name: GLib.Variant.new_string(app?.get_name() ?? ''),
                workspace: GLib.Variant.new_int32(ws ? ws.index() : -1),
                focused: GLib.Variant.new_boolean(w === focus),
                minimized: GLib.Variant.new_boolean(w.minimized),
            };
        });
    }

    ActivateWindow(id) {
        const w = this._find(id);
        if (!w)
            return false;
        const time = global.get_current_time();
        const ws = w.get_workspace();
        if (ws && ws !== global.workspace_manager.get_active_workspace())
            ws.activate_with_focus(w, time);
        else
            w.activate(time);
        return true;
    }

    CloseWindow(id) {
        const w = this._find(id);
        if (!w)
            return false;
        w.delete(global.get_current_time());
        return true;
    }

    // ------------------------------------------------------------ clipboard

    _onClipboardChanged() {
        St.Clipboard.get_default().get_text(St.ClipboardType.CLIPBOARD,
            (_cb, text) => {
                if (text === null || text === undefined)
                    return;
                this._dbus.emit_signal('ClipboardChanged',
                    GLib.Variant.new('(s)', [text]));
            });
    }

    GetClipboardAsync(_params, invocation) {
        St.Clipboard.get_default().get_text(St.ClipboardType.CLIPBOARD,
            (_cb, text) => {
                invocation.return_value(GLib.Variant.new('(s)', [text ?? '']));
            });
    }

    Paste(text) {
        St.Clipboard.get_default().set_text(St.ClipboardType.CLIPBOARD, text);
        if (this._pasteTimeout)
            GLib.source_remove(this._pasteTimeout);
        // The launcher hides itself before asking. Wait for focus to land
        // back on the previous window, then type the shortcut there.
        let tries = 0;
        this._pasteTimeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 40, () => {
            const focus = global.display.focus_window;
            const cls = (focus?.get_wm_class() ?? '').toLowerCase();
            const ready = focus && cls !== PARSEC_CLASS;
            tries += 1;
            if (!ready && tries < 25)
                return GLib.SOURCE_CONTINUE;
            this._pasteTimeout = 0;
            if (ready)
                this._typePaste(TERMINAL_CLASSES.has(cls));
            return GLib.SOURCE_REMOVE;
        });
    }

    _typePaste(shift) {
        if (!this._keyboard) {
            const seat = Clutter.get_default_backend().get_default_seat();
            this._keyboard = seat.create_virtual_device(
                Clutter.InputDeviceType.KEYBOARD_DEVICE);
        }
        const kb = this._keyboard;
        const press = key => kb.notify_keyval(
            GLib.get_monotonic_time(), key, Clutter.KeyState.PRESSED);
        const release = key => kb.notify_keyval(
            GLib.get_monotonic_time(), key, Clutter.KeyState.RELEASED);
        press(Clutter.KEY_Control_L);
        if (shift)
            press(Clutter.KEY_Shift_L);
        press(Clutter.KEY_v);
        release(Clutter.KEY_v);
        if (shift)
            release(Clutter.KEY_Shift_L);
        release(Clutter.KEY_Control_L);
    }
}

export default class ParsecExtension extends Extension {
    enable() {
        this._service = new Service();
    }

    disable() {
        this._service?.destroy();
        this._service = null;
    }
}
