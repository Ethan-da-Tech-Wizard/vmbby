#!/usr/bin/env python3
# Program: vmbby - vmbby_controller.py
# Programmer: Ethan-da-Tech-Wizard (written with Claude, an AI assistant)
# Last Modified: 2026-11-18
# Purpose: GTK4/libadwaita desktop window that animates a portal graphic and runs the vmbby command-line program when its buttons are clicked.

"""GTK4 controller for vmbby.

The Rust binary owns VM behavior. This helper gives the project a custom,
Wayland-friendly controller without pulling in Electron or a browser runtime.
"""

# Imports: math for the animation, shutil to find the vmbby program, subprocess to run it, sys for command-line arguments, threading so commands do not freeze the window, time for animation timing, Path for file paths.
import math
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

# gi is the Python binding to GTK. We must pick GTK version 4 and libadwaita version 1 BEFORE importing from gi.repository.
import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
# Import the GTK pieces used below: Adw (libadwaita app window), GLib (event loop helpers), Gdk (display and colors), Gio (file list helpers) and Gtk (widgets). "noqa: E402" tells the style checker the late import is intentional.
from gi.repository import Adw, GLib, Gdk, Gio, Gtk  # noqa: E402


# ROOT is the project folder (two levels above this file). BIN is the vmbby program: the one found on PATH if installed, otherwise the debug build inside ROOT.
ROOT = Path(__file__).resolve().parents[1]
BIN = shutil.which("vmbby") or str(ROOT / "target" / "debug" / "vmbby")


class Portal(Gtk.DrawingArea):
    """An animated drawing area that paints a colorful ripple/portal background with the vmbby title."""
    def __init__(self):
        """Set up the drawing area size, remember the start time and ask GTK to redraw every frame."""
        # Run the parent class setup first.
        super().__init__()
        # Remember when the animation began; the drawing uses elapsed time.
        self.start = time.monotonic()
        # Preferred size of the drawing area in pixels.
        self.set_content_width(840)
        self.set_content_height(310)
        # Tell GTK to call self.draw whenever the area needs painting.
        self.set_draw_func(self.draw)
        # Call self.tick on every animation frame.
        self.add_tick_callback(self.tick)

    def tick(self, _widget, _clock):
        """Request a repaint and keep the animation running."""
        self.queue_draw()
        # Returning CONTINUE keeps the callback active for the next frame.
        return GLib.SOURCE_CONTINUE

    def draw(self, _area, cr, width, height):
        """Paint one animation frame using the Cairo drawing context cr."""
        # t is seconds since the animation started; sine waves of t create the motion.
        t = time.monotonic() - self.start
        # cx and cy are the center of the drawing area.
        cx = width / 2
        cy = height / 2

        # Fill the whole background with a near-black purple.
        cr.set_source_rgb(0.014, 0.011, 0.026)
        cr.rectangle(0, 0, width, height)
        cr.fill()

        # Draw 58 colored rings; each has its own phase so the colors and sizes shift smoothly.
        for i in range(58):
            phase = t * 1.45 + i * 0.34
            radius = 12 + i * 7.4 + math.sin(phase) * 16
            red = 0.50 + 0.45 * math.sin(phase)
            green = 0.38 + 0.48 * math.sin(phase + 2.1)
            blue = 0.58 + 0.38 * math.sin(phase + 4.2)
            # Fainter rings further out (alpha never drops below 0.035).
            alpha = max(0.035, 0.40 - i * 0.006)
            cr.set_source_rgba(red, green, blue, alpha)
            cr.set_line_width(1.6 + 1.4 * math.sin(phase * 0.5))
            wobble_x = math.sin(phase * 0.72) * 32
            wobble_y = math.cos(phase * 0.84) * 20
            # Draw a circle slightly wobbling around the center, then stroke (outline) it.
            cr.arc(cx + wobble_x, cy + wobble_y, radius, 0, math.tau)
            cr.stroke()

        # Draw 26 wavy cyan horizontal lines across the area.
        for i in range(26):
            y = (i / 25) * height
            amp = 10 + 8 * math.sin(t + i)
            cr.set_source_rgba(0.15, 1.0, 0.88, 0.16)
            cr.set_line_width(1)
            cr.move_to(0, y)
            # Step across the width in 12 pixel jumps, bending each line with a sine wave.
            for x in range(0, width + 12, 12):
                cr.line_to(x, y + math.sin(x * 0.025 + t * 2.8 + i) * amp)
            cr.stroke()

        # Draw the title and subtitle text centered in the area.
        title = "vmbby"
        subtitle = "Arch host | Plasma/Hyprland Wayland | Ubuntu guest"
        cr.select_font_face("Sans", 0, 1)
        cr.set_font_size(68)
        ext = cr.text_extents(title)
        # First draw a dark shadow offset by 5 pixels, then the bright yellow title on top.
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.55)
        cr.move_to(cx - ext.width / 2 + 5, cy + 16 + 5)
        cr.show_text(title)
        cr.set_source_rgb(1.0, 0.98, 0.48)
        cr.move_to(cx - ext.width / 2, cy + 16)
        cr.show_text(title)

        # The subtitle uses a smaller monospace font in light cyan.
        cr.select_font_face("Monospace", 0, 1)
        cr.set_font_size(15)
        ext = cr.text_extents(subtitle)
        cr.set_source_rgb(0.70, 1.0, 0.94)
        cr.move_to(cx - ext.width / 2, cy + 46)
        cr.show_text(subtitle)


class Controller(Adw.Application):
    """The application: builds the window with buttons and an output box."""
    def __init__(self):
        """Register the application id and connect the activate signal to build the window."""
        # application_id is the unique name of this app.
        super().__init__(application_id="dev.ethan.vmbby")
        self.connect("activate", self.activate)

    def activate(self, app):
        """Build and show the whole window when the application starts."""
        # Create the main window and give it a title and starting size.
        win = Adw.ApplicationWindow(application=app)
        win.set_title("vmbby")
        win.set_default_size(940, 720)

        # Define the look of the app with CSS: colors, rounded buttons, a monospace output area and a status pill.
        css = Gtk.CssProvider()
        css.load_from_data(
            b"""
            window { background: #080711; }
            .panel { padding: 16px; background: #090817; }
            .strip { padding: 12px; background: #15142a; border-radius: 8px; }
            .status { color: #10211e; background: #effffb; font-family: monospace; font-size: 13px; }
            .headline { color: #fff892; font-weight: 900; font-size: 24px; }
            .subtle { color: #b7fff2; }
            .pill { color: #080711; background: #b7fff2; border-radius: 999px; padding: 5px 10px; font-weight: 800; }
            button { border-radius: 8px; font-weight: 800; padding: 10px 14px; color: #080711; background: #fff36d; }
            button:hover { background: #34ffe0; }
            .hot { background: #ff4bd8; color: #100713; }
            .go { background: #31ff9b; color: #07130d; }
            .danger { background: #ff604f; color: #160504; }
            entry { color: #effffb; background: #111022; border-radius: 8px; padding: 9px; }
            """
        )
        # Apply the CSS to every widget on this display.
        Gtk.StyleContext.add_provider_for_display(
            Gdk.Display.get_default(),
            css,
            Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION,
        )

        # The main vertical column that holds every section of the window.
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        box.add_css_class("panel")
        win.set_content(box)

        # Add the animated portal at the top.
        box.append(Portal())

        # Header strip: titles on the left and a status pill on the right.
        head = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=12)
        head.add_css_class("strip")
        box.append(head)

        # Two stacked labels: the headline and a short description.
        title_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        title = Gtk.Label(label="Ubuntu cockpit")
        title.set_xalign(0)
        title.add_css_class("headline")
        subtitle = Gtk.Label(label="Real Ubuntu VM in one controlled Wayland window")
        subtitle.set_xalign(0)
        subtitle.add_css_class("subtle")
        title_box.append(title)
        title_box.append(subtitle)
        title_box.set_hexpand(True)
        head.append(title_box)

        # The pill shows READY or RUNNING; it is stored on self so other methods can change it.
        self.state = Gtk.Label(label="READY")
        self.state.add_css_class("pill")
        head.append(self.state)

        # Row for choosing the Ubuntu installer file.
        iso_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        iso_row.add_css_class("strip")
        box.append(iso_row)
        # Text box where the ISO path is typed or filled in by the Browse button.
        self.iso_entry = Gtk.Entry()
        self.iso_entry.set_placeholder_text("/path/to/ubuntu-24.04.iso")
        self.iso_entry.set_hexpand(True)
        iso_row.append(self.iso_entry)
        # Browse button opens a file picker (browse_iso).
        browse = Gtk.Button(label="Browse ISO")
        browse.connect("clicked", self.browse_iso, win)
        iso_row.append(browse)
        # Install button starts the installer with the chosen ISO (install_clicked).
        install = Gtk.Button(label="Install Ubuntu")
        install.add_css_class("hot")
        install.connect("clicked", self.install_clicked)
        iso_row.append(install)

        # Row of command buttons.
        buttons = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        buttons.add_css_class("strip")
        box.append(buttons)
        # Each entry is (button text, vmbby sub-command, optional color class). Go and Stop are colored green and red.
        for label, command, css_class in [
            ("Doctor", ["doctor"], None),
            ("Init Disk", ["init"], None),
            ("Run Ubuntu", ["run"], "go"),
            ("Status", ["status"], None),
            ("Logs", ["logs"], None),
            ("Explain", ["explain"], None),
            ("Stop", ["stop"], "danger"),
        ]:
            # Create one button per entry and hook it to command_clicked, passing its command.
            btn = Gtk.Button(label=label)
            if css_class:
                btn.add_css_class(css_class)
            btn.connect("clicked", self.command_clicked, command)
            buttons.append(btn)

        # Read-only text area that shows the output of commands.
        self.output = Gtk.TextView()
        self.output.set_editable(False)
        self.output.set_monospace(True)
        self.output.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
        self.output.add_css_class("status")
        # Wrap the text area so it can scroll.
        scroller = Gtk.ScrolledWindow()
        scroller.set_vexpand(True)
        scroller.set_min_content_height(220)
        scroller.set_child(self.output)
        box.append(scroller)

        # Show first-time instructions in the output area.
        self.write(
            "vmbby controller ready\n"
            "1. Click Doctor.\n"
            "2. Click Init Disk.\n"
            "3. Pick the Ubuntu ISO and click Install Ubuntu.\n"
            "4. After Ubuntu is installed, click Run Ubuntu.\n\n"
        )
        # Show the window.
        win.present()

    def command_clicked(self, _button, command):
        """Handler for command buttons: run the matching vmbby sub-command."""
        self.run_command(command)

    def install_clicked(self, _button):
        """Handler for Install Ubuntu: needs an ISO path in the text box first."""
        # Read the path the user entered and trim spaces.
        iso = self.iso_entry.get_text().strip()
        # No path entered: show a hint and stop.
        if not iso:
            self.write("\nPick an Ubuntu ISO first. Example: ubuntu-24.04-desktop-amd64.iso\n")
            return
        self.run_command(["install", iso])

    def browse_iso(self, _button, win):
        """Open a file chooser limited to .iso files."""
        # Create the dialog and give it a title.
        dialog = Gtk.FileDialog()
        dialog.set_title("Choose Ubuntu ISO")
        # Create a filter that only shows files ending in .iso.
        iso_filter = Gtk.FileFilter()
        iso_filter.set_name("ISO images")
        iso_filter.add_pattern("*.iso")
        # GTK wants filters in a list model, so wrap the single filter in one.
        filters = Gio.ListStore.new(Gtk.FileFilter)
        filters.append(iso_filter)
        dialog.set_filters(filters)
        # Show the dialog; iso_chosen is called when the user finishes.
        dialog.open(win, None, self.iso_chosen)

    def iso_chosen(self, dialog, result):
        """Put the chosen file path into the text box, ignoring a simple cancel."""
        try:
            # Get the file the user picked (raises GLib.Error if the dialog was cancelled).
            file = dialog.open_finish(result)
            if file:
                self.iso_entry.set_text(file.get_path() or "")
        # If the dialog was just dismissed or cancelled, say nothing; report any other error.
        except GLib.Error as exc:
            message = str(exc).lower()
            if "dismiss" not in message and "cancel" not in message:
                self.write(f"ISO picker failed: {exc}\n")

    def run_command(self, command):
        """Show the command in the output, set the pill to RUNNING and run it on a background thread."""
        self.write(f"\n$ {BIN} {' '.join(command)}\n")
        self.state.set_text("RUNNING")
        # A background thread keeps the window responsive while vmbby runs; daemon=True lets the app exit even if it is still running.
        thread = threading.Thread(target=self.command_worker, args=(command,), daemon=True)
        thread.start()

    def command_worker(self, command):
        """Runs on the background thread: execute vmbby and collect its output."""
        try:
            # Run vmbby with the sub-command in the project folder, capturing stdout and stderr as text.
            proc = subprocess.run([BIN, *command], cwd=ROOT, text=True, capture_output=True, check=False)
            text = ""
            if proc.stdout:
                text += proc.stdout
            if proc.stderr:
                text += proc.stderr
            # A non-zero exit code means the command failed, so add it to the text.
            if proc.returncode != 0:
                text += f"[exit code {proc.returncode}]\n"
            # GTK widgets must only be changed from the main thread, so schedule finish_command there.
            GLib.idle_add(self.finish_command, text or "done\n")
        # If the program could not even start, show the error instead.
        except Exception as exc:
            GLib.idle_add(self.finish_command, f"failed: {exc}\n")

    def finish_command(self, text):
        """Runs on the main thread: show the output and set the pill back to READY."""
        self.write(text)
        self.state.set_text("READY")
        # Remove this one-time idle callback.
        return GLib.SOURCE_REMOVE

    def write(self, text):
        """Append text to the output area and scroll to the bottom."""
        # buf is the text buffer behind the text view.
        buf = self.output.get_buffer()
        end = buf.get_end_iter()
        buf.insert(end, text)
        # Place an invisible bookmark at the end and scroll to it.
        mark = buf.create_mark(None, buf.get_end_iter(), False)
        self.output.scroll_to_mark(mark, 0.0, True, 0.0, 1.0)


# Start the app when run directly; the program exits with the app's exit code.
if __name__ == "__main__":
    raise SystemExit(Controller().run(sys.argv))
