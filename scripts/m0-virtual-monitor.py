#!/usr/bin/env python3
"""M0 spike: does this machine stream a GNOME virtual monitor?

Asks Mutter for a virtual monitor through its ScreenCast D-Bus API (the same
call GNOME's RDP server makes), pulls frames from it with GStreamer, and
reports whether the monitor appeared and how many frames arrived.

    scripts/m0-virtual-monitor.py [--seconds N] [--dmabuf] [--connector NAME]

With --connector, records that real monitor instead (e.g. eDP-1), to tell a
virtual-monitor problem apart from a general screencast problem.

Nothing persists: the monitor disappears when the script exits.
"""

import argparse
import os
import sys
import threading

import gi

gi.require_version("Gst", "1.0")
from gi.repository import Gio, GLib, Gst

BUS = "org.gnome.Mutter.ScreenCast"
PATH = "/org/gnome/Mutter/ScreenCast"
CURSOR_EMBEDDED = 1


def call(bus, path, iface, method, args=None, reply="()"):
    return bus.call_sync(
        BUS, path, iface, method, args, GLib.VariantType(reply),
        Gio.DBusCallFlags.NONE, -1, None,
    ).unpack()


def connector_names(bus):
    state = bus.call_sync(
        "org.gnome.Mutter.DisplayConfig", "/org/gnome/Mutter/DisplayConfig",
        "org.gnome.Mutter.DisplayConfig", "GetCurrentState", None, None,
        Gio.DBusCallFlags.NONE, -1, None,
    ).unpack()
    return [monitor[0][0] for monitor in state[1]]


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--seconds", type=float, default=5)
    parser.add_argument("--dmabuf", action="store_true",
                        help="let GStreamer negotiate DMA-BUF instead of system memory")
    parser.add_argument("--connector", help="record this real monitor instead of a virtual one")
    args = parser.parse_args()

    Gst.init(None)
    bus = Gio.bus_get_sync(Gio.BusType.SESSION)
    before = connector_names(bus)

    (session,) = call(bus, PATH, BUS, "CreateSession",
                      GLib.Variant("(a{sv})", ({},)), "(o)")
    properties = {"cursor-mode": GLib.Variant("u", CURSOR_EMBEDDED)}
    if args.connector:
        (stream,) = call(bus, session, BUS + ".Session", "RecordMonitor",
                         GLib.Variant("(sa{sv})", (args.connector, properties)), "(o)")
    else:
        (stream,) = call(bus, session, BUS + ".Session", "RecordVirtual",
                         GLib.Variant("(a{sv})", (properties,)), "(o)")

    loop = GLib.MainLoop()
    result = {"frames": 0, "caps": None, "error": None, "node": None, "monitors": None}

    def on_frame(sink):
        sample = sink.emit("pull-sample")
        if result["caps"] is None:
            result["caps"] = sample.get_caps().to_string()
        result["frames"] += 1
        return Gst.FlowReturn.OK

    def on_message(_bus, message):
        if message.type == Gst.MessageType.ERROR:
            error, _debug = message.parse_error()
            result["error"] = error.message
            loop.quit()

    def finish():
        result["monitors"] = connector_names(bus)
        loop.quit()
        return False

    def on_node(_conn, _sender, _path, _iface, _signal, params):
        (node,) = params.unpack()
        result["node"] = node
        memory = "" if args.dmabuf else "video/x-raw ! "
        pipeline = Gst.parse_launch(
            f"pipewiresrc path={node} always-copy=true ! {memory}"
            "videoconvert ! appsink name=sink emit-signals=true max-buffers=2 drop=true sync=false"
        )
        pipeline.get_by_name("sink").connect("new-sample", on_frame)
        pipeline.get_bus().add_signal_watch()
        pipeline.get_bus().connect("message", on_message)
        pipeline.set_state(Gst.State.PLAYING)
        result["pipeline"] = pipeline
        GLib.timeout_add(int(args.seconds * 1000), finish)

    bus.signal_subscribe(BUS, BUS + ".Stream", "PipeWireStreamAdded", stream,
                         None, Gio.DBusSignalFlags.NONE, on_node)
    # pipewiresrc blocks the main thread when the stream never starts, so the
    # deadline runs on its own thread with its own bus connection.
    def watchdog():
        print("PipeWire node:     ", result["node"])
        print("RESULT: stream never started (consumer blocked connecting to PipeWire)")
        own_bus = Gio.bus_get_sync(Gio.BusType.SESSION)
        call(own_bus, session, BUS + ".Session", "Stop")
        sys.stdout.flush()
        os._exit(1)

    deadline = threading.Timer(args.seconds + 10, watchdog)
    deadline.daemon = True
    deadline.start()

    call(bus, session, BUS + ".Session", "Start")
    GLib.timeout_add(int(args.seconds * 1000) + 5000, finish)  # no node at all
    loop.run()

    deadline.cancel()
    if "pipeline" in result:
        result["pipeline"].set_state(Gst.State.NULL)
    call(bus, session, BUS + ".Session", "Stop")

    added = sorted(set(result["monitors"] or []) - set(before))
    print(f"PipeWire node:     {result['node']}")
    print(f"New monitor:       {', '.join(added) or 'none'}")
    print(f"Frames received:   {result['frames']} in {args.seconds:g} s")
    print(f"Negotiated format: {result['caps']}")
    if result["error"]:
        print(f"GStreamer error:   {result['error']}")
    ok = result["frames"] > 0
    source = args.connector or "the virtual monitor"
    print(f"RESULT: {source} streams" if ok else f"RESULT: no frames from {source}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
