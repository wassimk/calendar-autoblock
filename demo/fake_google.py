#!/usr/bin/env python3
"""Stands in for Google's Calendar API and token endpoint so the demo never uses a real account."""
import json
import sys
import urllib.parse
from datetime import datetime, timedelta
from http.server import BaseHTTPRequestHandler, HTTPServer
from zoneinfo import ZoneInfo

CT = ZoneInfo("America/Chicago")
APPOINTMENTS = "appointments@group.calendar.google.com"
CALENDARS = [
    {"id": "you@example.com", "summary": "Personal", "accessRole": "owner", "primary": True},
    {"id": APPOINTMENTS, "summary": "Appointments", "accessRole": "owner"},
    {"id": "family@group.calendar.google.com", "summary": "Family", "accessRole": "owner"},
    {"id": "shared@group.calendar.google.com", "summary": "Book club", "accessRole": "reader"},
]


def next_weekday(weekday):
    today = datetime.now(CT).date()
    return today + timedelta(days=(weekday - today.weekday() - 1) % 7 + 1)


def timed(eid, title, weekday, start, minutes, **extra):
    day = next_weekday(weekday)
    begin = datetime.combine(day, datetime.strptime(start, "%H:%M").time(), CT)
    end = begin + timedelta(minutes=minutes)
    return {"id": eid, "status": "confirmed", "summary": title, "iCalUID": f"{eid}@demo",
            "start": {"dateTime": begin.isoformat(), "timeZone": "America/Chicago"},
            "end": {"dateTime": end.isoformat(), "timeZone": "America/Chicago"}, **extra}


# Titles never reach the work calendar or the output; they're here to show what gets filtered
SOURCE = [
    timed("dentist", "Dentist", 1, "10:00", 60),
    timed("pt", "Physical therapy", 2, "17:30", 45),
    timed("conference", "Parent-teacher conference", 3, "07:15", 30),
    timed("dinner", "Dinner with friends", 2, "19:00", 120),
    timed("lunch", "Lunch maybe", 3, "12:00", 60, transparency="transparent"),
    timed("invite", "Unanswered invite", 0, "14:00", 60,
          attendees=[{"email": "you@example.com", "self": True, "responseStatus": "needsAction"}]),
    timed("brunch", "Saturday brunch", 5, "10:00", 90),
]
COPIES = {}
NEXT_ID = [0]


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, body, status=200):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def parts(self):
        url = urllib.parse.urlparse(self.path)
        return [urllib.parse.unquote(p) for p in url.path.strip("/").split("/")], urllib.parse.parse_qs(url.query)

    def body(self):
        return json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")

    def do_GET(self):
        path, query = self.parts()
        if path == ["users", "me", "calendarList"]:
            return self.reply({"items": CALENDARS})
        if len(path) == 3 and path[0] == "calendars" and path[2] == "events":
            if "privateExtendedProperty" in query:
                return self.reply({"items": list(COPIES.get(path[1], {}).values())})
            return self.reply({"items": SOURCE if path[1] == APPOINTMENTS else []})
        self.reply({}, 404)

    def do_POST(self):
        path, _ = self.parts()
        if path == ["token"]:
            self.rfile.read(int(self.headers.get("Content-Length", 0)))
            return self.reply({"access_token": "demo", "expires_in": 3600})
        event = self.body()
        NEXT_ID[0] += 1
        event["id"] = f"copy{NEXT_ID[0]}"
        COPIES.setdefault(path[1], {})[event["id"]] = event
        self.reply(event)

    def do_PATCH(self):
        path, _ = self.parts()
        event = {**COPIES[path[1]][path[3]], **self.body()}
        COPIES[path[1]][path[3]] = event
        self.reply(event)

    def do_DELETE(self):
        path, _ = self.parts()
        COPIES.get(path[1], {}).pop(path[3], None)
        self.send_response(204)
        self.end_headers()


if __name__ == "__main__":
    HTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
