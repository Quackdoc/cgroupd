#!/usr/bin/env python3
"""Minimal org.freedesktop.systemd1 shim on the *session* bus.

Emulates the bits of the systemd user manager that desktop apps (Konsole,
KDE's KProcessRunner, libcosmic, GNOME) use to move processes into
transient scopes: Subscribe, StartTransientUnit (PIDs property),
AttachProcessesToUnit, StopUnit/KillUnit, JobRemoved. Placement is done
by cgroupd's system-bus CreateApp.

Run per user session, e.g. from the desktop autostart / init user service:
    scripts/systemd-shim.py [--bus session|system] [--verbose]
"""
import argparse
import itertools
import json
import os
import re
import sys

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

BUS_NAME = "org.freedesktop.systemd1"
PATH = "/org/freedesktop/systemd1"
MGR = "org.freedesktop.systemd1.Manager"
CG = ("org.cgroupd", "/org/cgroupd/Manager", "org.cgroupd.Manager")

VERBOSE = False


def log(*a):
	if VERBOSE:
		print("systemd-shim:", *a, file=sys.stderr, flush=True)


def unit_to_aid(name):
	base = re.sub(r"\.(scope|service)$", "", name)
	return (re.sub(r"[^A-Za-z0-9_-]", "_", base)[:64]) or "app"


def esc_path(name):
	return "/org/freedesktop/systemd1/unit/" + re.sub(
		r"[^A-Za-z0-9]", lambda m: "_%02x" % ord(m.group(0)), name
	)


class Shim(dbus.service.Object):
	def __init__(self, bus, cg_bus):
		super().__init__(bus, PATH)
		self.bus = bus
		self.cg_bus = cg_bus
		self.job_ids = itertools.count(1)
		self.units = {}  # name -> aid
		self.subscribed = 0

	def cgroupd(self):
		obj = self.cg_bus.get_object(CG[0], CG[1])
		return dbus.Interface(obj, CG[2])

	def cg_call(self, method, *args):
		reply = getattr(self.cgroupd(), method)(*args)
		payload = json.loads(str(reply))
		if payload.get("status") != "ok":
			raise dbus.DBusException(
				"cgroupd: %s" % payload.get("msg", payload),
				name="org.freedesktop.systemd1.Failed",
			)
		return payload.get("detail", "")

	def place(self, name, pids):
		aid = unit_to_aid(name)
		self.cg_call("EnsureUser")
		for pid in pids:
			self.cg_call("CreateApp", aid, dbus.UInt32(pid))
		self.units[name] = aid
		log("placed", pids, "in app/%s" % aid)
		return aid

	# -- Manager methods ----------------------------------------------
	@dbus.service.method(MGR, in_signature="", out_signature="")
	def Subscribe(self):
		self.subscribed += 1

	@dbus.service.method(MGR, in_signature="", out_signature="")
	def Unsubscribe(self):
		self.subscribed = max(0, self.subscribed - 1)

	@dbus.service.method(MGR, in_signature="ssa(sv)a(sa(sv))", out_signature="o")
	def StartTransientUnit(self, name, mode, props, aux):
		log("StartTransientUnit", name, mode, [str(p[0]) for p in props])
		pids = []
		for key, val in props:
			if str(key) == "PIDs":
				pids += [int(p) for p in val]
		if not pids:
			raise dbus.DBusException(
				"shim: only PIDs-based transient units are supported",
				name="org.freedesktop.DBus.Error.NotSupported",
			)
		if name in self.units:
			raise dbus.DBusException(
				"Unit %s already exists" % name,
				name="org.freedesktop.systemd1.UnitExists",
			)
		self.place(name, pids)
		jid = next(self.job_ids)
		job = "/org/freedesktop/systemd1/job/%d" % jid
		GLib.idle_add(self._job_done, jid, job, name)
		return dbus.ObjectPath(job)

	def _job_done(self, jid, job, name):
		self.JobRemoved(dbus.UInt32(jid), dbus.ObjectPath(job), name, "done")
		return False

	@dbus.service.method(MGR, in_signature="ssau", out_signature="")
	def AttachProcessesToUnit(self, unit, subcgroup, pids):
		self.place(unit, [int(p) for p in pids])

	@dbus.service.method(MGR, in_signature="s", out_signature="o")
	def GetUnit(self, name):
		if name not in self.units:
			raise dbus.DBusException(
				"Unit %s not loaded" % name,
				name="org.freedesktop.systemd1.NoSuchUnit",
			)
		return dbus.ObjectPath(esc_path(name))

	@dbus.service.method(MGR, in_signature="ss", out_signature="o")
	def StopUnit(self, name, mode):
		aid = self.units.pop(name, None)
		if aid is None:
			raise dbus.DBusException(
				"Unit %s not loaded" % name,
				name="org.freedesktop.systemd1.NoSuchUnit",
			)
		self.cg_call("StopApp", aid, dbus.UInt32(2))
		jid = next(self.job_ids)
		job = "/org/freedesktop/systemd1/job/%d" % jid
		GLib.idle_add(self._job_done, jid, job, name)
		return dbus.ObjectPath(job)

	@dbus.service.method(MGR, in_signature="ssi", out_signature="")
	def KillUnit(self, name, who, sig):
		self.StopUnit(name, "replace")

	@dbus.service.method(MGR, in_signature="s", out_signature="")
	def ResetFailedUnit(self, name):
		pass

	@dbus.service.signal(MGR, signature="uoss")
	def JobRemoved(self, jid, job, unit, result):
		pass

	# -- Properties ---------------------------------------------------
	PROPS = {
		"Version": "257 (cgroupd-shim)",
		"Features": "",
		"Virtualization": "",
		"Architecture": "x86-64",
	}

	@dbus.service.method(dbus.PROPERTIES_IFACE, in_signature="ss", out_signature="v")
	def Get(self, iface, prop):
		if prop in self.PROPS:
			return dbus.String(self.PROPS[prop])
		raise dbus.DBusException(
			"Unknown property %s" % prop,
			name="org.freedesktop.DBus.Error.UnknownProperty",
		)

	@dbus.service.method(dbus.PROPERTIES_IFACE, in_signature="s", out_signature="a{sv}")
	def GetAll(self, iface):
		return dbus.Dictionary({k: dbus.String(v) for k, v in self.PROPS.items()}, signature="sv")


def main():
	global VERBOSE
	ap = argparse.ArgumentParser()
	ap.add_argument("--bus", choices=("session", "system"), default="session")
	ap.add_argument("--verbose", "-v", action="store_true")
	args = ap.parse_args()
	VERBOSE = args.verbose
	DBusGMainLoop(set_as_default=True)
	bus = dbus.SessionBus() if args.bus == "session" else dbus.SystemBus()
	cg_bus = dbus.SystemBus(private=True)
	reply = bus.request_name(BUS_NAME, dbus.bus.NAME_FLAG_DO_NOT_QUEUE)
	if reply not in (dbus.bus.REQUEST_NAME_REPLY_PRIMARY_OWNER, dbus.bus.REQUEST_NAME_REPLY_ALREADY_OWNER):
		sys.exit("systemd-shim: %s already owned (real systemd --user running?)" % BUS_NAME)
	Shim(bus, cg_bus)
	log("serving", BUS_NAME, "on", args.bus, "bus, pid", os.getpid())
	GLib.MainLoop().run()


if __name__ == "__main__":
	main()
