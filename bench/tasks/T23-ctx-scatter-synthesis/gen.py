#!/usr/bin/env python3
"""Generator of setup/ for T23-ctx-scatter-synthesis.

    python3 bench/tasks/T23-ctx-scatter-synthesis/gen.py          # rewrite setup/
    python3 bench/tasks/T23-ctx-scatter-synthesis/gen.py --check  # compare, exit 1 on drift

Deterministic (fixed seed, no network, no clock). Lives next to setup/, not
inside it: the runner copies setup/ into the sandbox as is, and this file
spells out the answer.

setup/custody/day-01.log .. day-08.log: the custody log of a biobank's six
freezers over eight duty days. A small inventory simulation drives the noise
(receipts, moves, pulls and returns, disposals, audits, temperature rounds,
voided tickets, typo corrections, a box relocation), so every noise entry is
consistent with the state it describes. The planted chain is written at fixed
times on top of it and is never touched by the noise:

  day 01  KESTREL intake. S-0736 arrives with a sender's "REF" sticker (the
          naive first find); S-0731, 1.0 mL serum, goes to F2/R4/B12/B7. The
          handover corrects the sticker: the reference aliquot is S-0731.
  day 02  Dr. Ivers asks to move the KESTREL tubes out of F2.
  day 03  MOVE S-0731 -> F3/R1/B05/A2 (and three working aliquots), TR-2291.
  day 04  decoy S-0713 leaves F2/R4/B11 for F1/R3/B02/D4.
  day 06  audit finds F3/R1/B05 empty; TR-2291 is VOID, the tubes never left
          their original slots (the void names the ticket, not the samples).
  day 07  a PLAN to move F2/R4 boxes B09-B12 into F6/R3 B01-B04 on 10-25.
  day 08  F2 compressor fails at 02:10; EM-07 moves the F2/R4 boxes B09-B12
          whole into F5/R2 B21-B24 (B12 -> B24). At 06:40 a correction: the
          labels of the old B11 and B12 were swapped when shelved, the old
          B12 is in F5/R2/B23. Positions inside boxes never change.

Answer: F5/R2/B23/B7.
"""

from __future__ import annotations

import random
import shutil
import sys
import tempfile
from datetime import datetime, timedelta
from pathlib import Path

HERE = Path(__file__).resolve().parent
SETUP = HERE / "setup"
ANSWER = "F5/R2/B23/B7"

SEED = 0x7223
DAYS = 8
START = datetime(2026, 10, 12, 7, 0)
ROWS = "ABCDEFGHI"
FREEZERS = range(1, 7)
RACKS = range(1, 5)
BOXES = range(1, 13)

STUDIES = ["HERON", "OSPREY", "PLOVER", "WREN", "LARK", "SWIFT"]
MATERIALS = [
    ("serum", "1x0.5mL"), ("serum", "2x0.5mL"), ("plasma", "1x0.5mL"),
    ("plasma", "2x1.0mL"), ("urine", "2x1.0mL"), ("DNA", "1x0.2mL"),
    ("RNA", "1x0.1mL"), ("PBMC", "1x1.0mL"), ("saliva", "1x1.0mL"),
    ("CSF", "1x0.5mL"),
]
COURIERS = ["DHL", "TNT", "UPS", "MED", "BIO"]
REQUESTERS = ["Dr. Hale", "Dr. Ruiz", "Dr. Nakamura", "Dr. Osei", "Dr. Lindqvist", "Dr. Moreau"]
LEADS = [
    ("R. Okafor", "J. Lind"), ("M. Varga", "T. Chen"), ("S. Dube", "P. Novak"),
    ("R. Okafor", "T. Chen"), ("M. Varga", "J. Lind"), ("A. Brandt", "S. Dube"),
    ("P. Novak", "R. Okafor"), ("T. Chen", "M. Varga"),
]

PLANTED_SAMPLES = {"S-0731", "S-0732", "S-0733", "S-0734", "S-0735", "S-0736", "S-0713"}
# Digit shuffles of 0731 other than the planted decoy would make grep noisy.
EXCLUDED_SAMPLES = PLANTED_SAMPLES | {"S-0137", "S-0173", "S-0317", "S-0371"}

KESTREL_BOX = (2, 4, 12)
TR2291_BOX = (3, 1, 5)
F2_R4 = [(2, 4, b) for b in (9, 10, 11, 12)]
EM07_TARGET = [(5, 2, b) for b in (21, 22, 23, 24)]
PLAN_TARGET = [(6, 3, b) for b in (1, 2, 3, 4)]
F4_DEFROST = [(4, 1, b) for b in (1, 2, 3)]
F6_SPARE = [(6, 1, b) for b in (1, 2, 3)]

# Boxes the noise never reads or writes. The F6/R1 spares stop being
# protected once the F4 boxes land in them on day 04.
PROTECTED_BOXES = set(F2_R4) | {TR2291_BOX} | set(EM07_TARGET) | set(PLAN_TARGET) | set(F4_DEFROST) | set(F6_SPARE)
# Boxes that start empty.
EMPTY_BOXES = {KESTREL_BOX, TR2291_BOX} | set(EM07_TARGET) | set(PLAN_TARGET) | set(F6_SPARE)


def slot_str(slot) -> str:
    f, r, b, pos = slot
    return f"F{f}/R{r}/B{b:02d}/{pos}"


def box_str(box) -> str:
    f, r, b = box
    return f"F{f}/R{r}/B{b:02d}"


def stamp(when: datetime) -> str:
    return when.strftime("%m-%d %H:%M")


def entry(when: datetime, kind: str, text: str) -> str:
    return f"{stamp(when)} {kind:<5} {text}"


class Bank:
    """Physical state of the freezers plus what the noise may touch."""

    def __init__(self, rng: random.Random) -> None:
        self.rng = rng
        self.slot_of: dict[str, tuple] = {}
        self.at: dict[tuple, str] = {}
        self.locked: set[str] = set(PLANTED_SAMPLES)
        self.reserved: set[tuple] = set()
        self.protected = set(PROTECTED_BOXES)
        self.study: dict[str, str] = {}
        free_ids = [f"S-{n:04d}" for n in range(1, 3000)]
        self.unused_ids = [s for s in free_ids if s not in EXCLUDED_SAMPLES]
        rng.shuffle(self.unused_ids)

    # -- primitives ---------------------------------------------------------

    def place(self, sample: str, slot: tuple) -> None:
        assert slot not in self.at, (sample, slot)
        self.slot_of[sample] = slot
        self.at[slot] = sample

    def take(self, sample: str) -> tuple:
        slot = self.slot_of.pop(sample)
        del self.at[slot]
        return slot

    def box_count(self, box: tuple) -> int:
        return sum(1 for slot in self.at if slot[:3] == box)

    def move_box(self, src: tuple, dst: tuple) -> None:
        moving = [(slot, sample) for slot, sample in self.at.items() if slot[:3] == src]
        for slot, sample in moving:
            del self.at[slot]
        for slot, sample in moving:
            new = dst + (slot[3],)
            self.place(sample, new)

    # -- noise helpers --------------------------------------------------------

    def noise_boxes(self) -> list[tuple]:
        return [(f, r, b) for f in FREEZERS for r in RACKS for b in BOXES if (f, r, b) not in self.protected]

    def free_slot(self) -> tuple:
        boxes = self.noise_boxes()
        while True:
            box = self.rng.choice(boxes)
            pos = f"{self.rng.choice(ROWS)}{self.rng.randrange(1, 10)}"
            slot = box + (pos,)
            if slot not in self.at and slot not in self.reserved:
                return slot

    def free_slot_in(self, box: tuple) -> tuple | None:
        options = [box + (f"{row}{col}",) for row in ROWS for col in range(1, 10)]
        options = [s for s in options if s not in self.at and s not in self.reserved]
        return self.rng.choice(options) if options else None

    def movable(self) -> list[str]:
        return sorted(
            sample for sample, slot in self.slot_of.items()
            if sample not in self.locked and slot[:3] not in self.protected
        )

    def new_id(self) -> str:
        return self.unused_ids.pop()

    def populate(self, count: int) -> None:
        boxes = [(f, r, b) for f in FREEZERS for r in RACKS for b in BOXES if (f, r, b) not in EMPTY_BOXES]
        boxes += [(2, 4, 9), (2, 4, 10), (2, 4, 11)]
        boxes = sorted(set(boxes) - EMPTY_BOXES)
        placed = 0
        while placed < count:
            box = self.rng.choice(boxes)
            pos = f"{self.rng.choice(ROWS)}{self.rng.randrange(1, 10)}"
            slot = box + (pos,)
            if slot in self.at or slot == (2, 4, 11, "C3"):
                continue
            sample = self.new_id()
            self.place(sample, slot)
            self.study[sample] = self.rng.choice(STUDIES)
            placed += 1


class Day:
    """One duty day: noise around planted events, in time order."""

    def __init__(self, number: int, bank: Bank, rng: random.Random, tickets: list[int]) -> None:
        self.number = number
        self.bank = bank
        self.rng = rng
        self.start = START + timedelta(days=number - 1)
        self.end = self.start + timedelta(hours=23, minutes=50)
        self.lines: list[str] = []
        self.planted: list[tuple[datetime, callable]] = []
        self.ticket_pool = tickets
        self.stats = {"RCV": 0, "MOVE": 0, "PULL": 0, "DISP": 0, "AUDIT": 0}

    def at(self, hour: int, minute: int, day_offset: int = 0) -> datetime:
        base = self.start.replace(hour=0, minute=0) + timedelta(days=day_offset)
        return base + timedelta(hours=hour, minutes=minute)

    def plant(self, when: datetime, action) -> None:
        self.planted.append((when, action))

    def emit(self, when: datetime, kind: str, text: str) -> None:
        self.lines.append(entry(when, kind, text))

    def ticket(self) -> str:
        return f"TR-{self.ticket_pool.pop(0)}"


class Log:
    def __init__(self) -> None:
        self.rng = random.Random(SEED)
        self.bank = Bank(random.Random(SEED + 1))
        self.bank.populate(1300)
        self.pending: list[tuple[datetime, str, object]] = []  # (due, kind, payload)
        self.designated: set[str] = set()
        self.files: dict[str, str] = {}

    # ---------------------------------------------------------------- noise

    def noise_entry(self, day: Day, when: datetime) -> None:
        rng, bank = self.rng, self.bank
        if day.number == DAYS and when >= day.at(1, 30, 1):
            # The last hours belong to the emergency: keep the noise thin.
            kinds, weights = ["AUDIT", "NOTE", "PULL"], [3, 2, 1]
        else:
            kinds = ["RCV", "MOVE", "PULL", "DISP", "AUDIT", "NOTE"]
            weights = [18, 18, 13, 5, 9, 7]
        kind = rng.choices(kinds, weights)[0]
        movable = bank.movable()

        if kind == "RCV":
            sample = bank.new_id()
            study = rng.choice(STUDIES)
            bank.study[sample] = study
            material, volume = rng.choice(MATERIALS)
            slot = bank.free_slot()
            courier = f"{rng.choice(COURIERS)}-{rng.randrange(5000, 5999)}"
            typo = rng.random() < 0.06 and not self.late(day, when)
            logged = slot
            if typo:
                logged = bank.free_slot_in(slot[:3]) or slot
            bank.place(sample, slot)
            day.emit(when, "RCV", f"{sample}  {material} {volume} -> {slot_str(logged)}  study {study}, courier {courier}")
            day.stats["RCV"] += 1
            if logged != slot:
                self.schedule_typo(when, sample, logged, slot)
        elif kind == "MOVE" and movable:
            sample = rng.choice(movable)
            src = bank.slot_of[sample]
            dst = bank.free_slot()
            ticket = day.ticket()
            phantom = rng.random() < 0.07 and not self.late(day, when)
            day.emit(when, "MOVE", f"{sample}  {slot_str(src)} -> {slot_str(dst)}  ticket {ticket}")
            day.stats["MOVE"] += 1
            if phantom:
                bank.locked.add(sample)
                bank.reserved.add(dst)
                due = when + timedelta(hours=rng.randrange(5, 30), minutes=rng.randrange(60))
                self.pending.append((due, "VOID", (ticket, sample, src, dst)))
            else:
                bank.take(sample)
                bank.place(sample, dst)
        elif kind == "PULL" and movable and when < day.end - timedelta(hours=4):
            sample = rng.choice(movable)
            slot = bank.slot_of[sample]
            assay = f"AS-{rng.randrange(300, 499)}"
            who = rng.choice(REQUESTERS)
            bank.locked.add(sample)
            day.emit(when, "PULL", f"{sample}  {slot_str(slot)}  for assay {assay} ({who})")
            day.stats["PULL"] += 1
            due = when + timedelta(minutes=rng.randrange(25, 240))
            self.pending.append((due, "RET", (sample, slot, assay)))
        elif kind == "DISP" and movable:
            sample = rng.choice(movable)
            slot = bank.take(sample)
            day.emit(when, "DISP", f"{sample}  {slot_str(slot)}  consumed by AS-{rng.randrange(300, 499)}")
            day.stats["DISP"] += 1
        elif kind == "AUDIT":
            clean = [
                box for box in bank.noise_boxes()
                if not any(s[:3] == box for s in bank.reserved)
                and not any(bank.slot_of.get(x, (0,))[:3] == box for x in bank.locked)
                and bank.box_count(box) > 0
            ]
            box = rng.choice(clean)
            count = bank.box_count(box)
            day.emit(when, "AUDIT", f"{box_str(box)}  {count} tubes, matches inventory")
            day.stats["AUDIT"] += 1
        else:
            day.emit(when, "NOTE", self.note(day, when))

    def late(self, day: Day, when: datetime) -> bool:
        return day.number == DAYS and when >= day.at(18, 0)

    def schedule_typo(self, when: datetime, sample: str, logged: tuple, actual: tuple) -> None:
        self.bank.locked.add(sample)
        self.bank.reserved.add(logged)
        due = when + timedelta(hours=self.rng.randrange(1, 14), minutes=self.rng.randrange(60))
        self.pending.append((due, "CORR", (when, sample, logged, actual)))

    def resolve(self, day: Day, when: datetime, kind: str, payload) -> None:
        bank, rng = self.bank, self.rng
        if kind == "RET":
            sample, slot, assay = payload
            day.emit(when, "RET", f"{sample}  -> {slot_str(slot)}  after {assay}")
            bank.locked.discard(sample)
        elif kind == "VOID":
            ticket, sample, src, dst = payload
            style = rng.randrange(3)
            if style == 0:
                text = (f"ticket {ticket}: entered but never carried out, {sample} is still in "
                        f"{slot_str(src)}; inventory reverted")
            elif style == 1:
                text = (f"ticket {ticket} cancelled by the requester before the move; "
                        f"inventory reverted, {box_str(dst[:3])} untouched")
            else:
                text = (f"ticket {ticket}: audit found {slot_str(dst)} empty, the tube never left "
                        f"its original slot; ticket voided, inventory reverted")
            day.emit(when, "VOID", text)
            bank.locked.discard(sample)
            bank.reserved.discard(dst)
        elif kind == "CORR":
            logged_at, sample, logged, actual = payload
            day.emit(when, "CORR", f"entry of {stamp(logged_at)} for {sample}: the slot is "
                                   f"{slot_str(actual)}, not {slot_str(logged)} (typo)")
            bank.locked.discard(sample)
            bank.reserved.discard(logged)

    def note(self, day: Day, when: datetime) -> str:
        rng, bank = self.rng, self.bank
        samples = bank.movable()
        s1, s2 = rng.sample(samples, 2)
        box = rng.choice(bank.noise_boxes())
        templates = [
            f"label printer on bench {rng.randrange(1, 4)} jammed; labels for {s1} and {s2} reprinted",
            f"courier {rng.choice(COURIERS)}-{rng.randrange(5000, 5999)} arrived {rng.randrange(10, 90)} min late, dry ice topped up",
            f"door alarm on F{rng.choice([1, 3, 4, 5, 6])} acknowledged after {rng.randrange(20, 90)} s, cabinet stayed below -75C",
            f"LN2 delivery received, tank at {rng.randrange(60, 98)}%",
            f"{rng.choice(REQUESTERS)} asked for the {rng.choice(STUDIES)} aliquot list; exported from LIMS",
            f"{s1} found rotated in its slot during a pick; no change of slot",
            f"spill kit restocked, size M gloves running low",
            f"assay AS-{rng.randrange(300, 499)} results due {(day.start + timedelta(days=rng.randrange(2, 9))).strftime('%m-%d')}",
            f"{box_str(box)} lid cracked, box replaced like for like, positions unchanged",
            f"barcode of {s2} unreadable, relabelled from LIMS record",
            f"training: {rng.choice(['K. Ahmed', 'L. Brooks', 'E. Sato'])} shadowed the duty lead on receipts",
        ]
        choice = rng.choice(templates + ["designation"])
        if choice != "designation":
            return choice
        # Other studies get a reference aliquot too, once each: "reference"
        # alone does not lead to KESTREL.
        open_studies = [s for s in ("HERON", "OSPREY", "WREN") if s not in self.designated]
        if not open_studies or not 8 <= when.hour < 18:
            return templates[0]
        study = open_studies[0]
        self.designated.add(study)
        return f"{rng.choice(REQUESTERS)} designated {s1} as the {study} reference aliquot"

    # ---------------------------------------------------------------- planted

    def planted_for(self, day: Day) -> None:
        bank = self.bank
        n = day.number

        def rcv(sample, material, volume, slot, study, courier, extra=""):
            def action(when):
                bank.place(sample, slot)
                bank.study[sample] = study
                day.emit(when, "RCV", f"{sample}  {material} {volume} -> {slot_str(slot)}  study {study}, courier {courier}{extra}")
                day.stats["RCV"] += 1
            return action

        def note(text):
            return lambda when: day.emit(when, "NOTE", text)

        if n == 1:
            day.plant(day.at(9, 52), rcv("S-0731", "serum", "1x1.0mL", KESTREL_BOX + ("B7",), "KESTREL", "MED-2207"))
            day.plant(day.at(9, 54), rcv("S-0732", "serum", "1x0.5mL", KESTREL_BOX + ("B8",), "KESTREL", "MED-2207"))
            day.plant(day.at(9, 56), rcv("S-0733", "serum", "1x0.5mL", KESTREL_BOX + ("B9",), "KESTREL", "MED-2207"))
            day.plant(day.at(9, 58), rcv("S-0734", "plasma", "1x0.5mL", KESTREL_BOX + ("C1",), "KESTREL", "MED-2207"))
            day.plant(day.at(10, 1), rcv("S-0736", "serum", "1x0.5mL", (3, 2, 14, "E6"), "KESTREL", "MED-2207",
                                         ", tube carries the sender's sticker \"REF\""))
            day.plant(day.at(10, 6), note("MED-2207 manifest lists S-0735 but the tube is not in the shipper; "
                                          "sender confirms it was never drawn"))
            day.plant(day.at(10, 9), note("KESTREL (Dr. M. Ivers): S-0736 is marked \"REF\" by the sender, so it "
                                          "is logged as the study reference aliquot for now"))
            day.plant(day.at(10, 40), rcv("S-0713", "urine", "2x1.0mL", (2, 4, 11, "C3"), "HERON", "DHL-5530"))
        elif n == 2:
            def pull(when):
                day.emit(when, "PULL", "S-0732  F2/R4/B12/B8  for assay AS-418 (Dr. Ivers)")
            def ret(when):
                day.emit(when, "RET", "S-0732  -> F2/R4/B12/B8  after AS-418")
            day.plant(day.at(11, 10), pull)
            day.plant(day.at(13, 45), ret)
            day.plant(day.at(15, 30), note("Dr. Ivers asked to take the KESTREL tubes out of F2 (too much door "
                                           "traffic) and keep them together in F3; space in F3/R1 to be checked"))
        elif n == 3:
            moves = [("S-0731", "B7", "A2"), ("S-0732", "B8", "A3"), ("S-0733", "B9", "A4"), ("S-0734", "C1", "A5")]
            for index, (sample, src, dst) in enumerate(moves):
                def move(when, sample=sample, src=src, dst=dst):
                    # Logged only: day 06 voids TR-2291, the tubes never moved.
                    day.emit(when, "MOVE", f"{sample}  {slot_str(KESTREL_BOX + (src,))} -> "
                                           f"{slot_str(TR2291_BOX + (dst,))}  ticket TR-2291")
                    day.stats["MOVE"] += 1
                day.plant(day.at(14, 5 + index), move)
        elif n == 4:
            def decoy(when):
                bank.take("S-0713")
                bank.place("S-0713", (1, 3, 2, "D4"))
                day.emit(when, "MOVE", "S-0713  F2/R4/B11/C3 -> F1/R3/B02/D4  ticket TR-2318")
                day.stats["MOVE"] += 1
            day.plant(day.at(9, 20), decoy)
            for index, (src, dst) in enumerate(zip(F4_DEFROST, F6_SPARE)):
                def box_move(when, src=src, dst=dst):
                    bank.move_box(src, dst)
                    bank.protected.discard(dst)
                    day.emit(when, "MOVE", f"box {box_str(src)} -> {box_str(dst)}  whole box, positions unchanged "
                                           f"(MT-12, F4 defrost)")
                day.plant(day.at(13, 2 + 3 * index), box_move)
            day.plant(day.at(13, 15), note("F4 defrost started; F4/R1 boxes B01-B03 stay in F6/R1 for good, "
                                           "LIMS updated under MT-12"))
        elif n == 5:
            day.plant(day.at(16, 5), note("Dr. Ivers (email): the KESTREL reference stays frozen until the "
                                          "2027 re-assay, no pulls of it without her written request"))
        elif n == 6:
            def audit(when):
                day.emit(when, "AUDIT", "F3/R1/B05  0 tubes, inventory expects 4 (TR-2291); escalated to the supervisor")
            def void(when):
                day.emit(when, "VOID", "ticket TR-2291 (supervisor A. Brandt): the move was entered in LIMS but "
                                       "never carried out, every tube on the ticket is still in the slot it had "
                                       "before; inventory reverted for all of them")
            day.plant(day.at(8, 10), audit)
            day.plant(day.at(8, 35), void)
        elif n == 7:
            day.plant(day.at(11, 0), note("PLAN: facilities booked the F2 compressor service for 10-26; on 10-25 "
                                          "the F2/R4 boxes B09-B12 go whole into F6/R3 as B01-B04"))
        elif n == 8:
            day.plant(day.at(2, 10, 1), lambda when: day.emit(
                when, "ALARM", "F2 compressor failure, cabinet at -61C and rising; on-call lead called in"))
            for index, (src, dst) in enumerate(zip(F2_R4, EM07_TARGET)):
                def em_move(when, src=src, dst=dst):
                    day.emit(when, "MOVE", f"box {box_str(src)} -> {box_str(dst)}  whole box, positions "
                                           f"unchanged (EM-07, emergency)")
                day.plant(day.at(2, 24 + 2 * index, 1), em_move)
            day.plant(day.at(2, 40, 1), note("F2 emptied and switched off; the 10-25 move to F6/R3 is cancelled, "
                                             "the F2/R4 boxes stay in F5/R2"))

            def swap_audit(when):
                # The physical truth: old B11 sits under the B24 label, old B12 under B23.
                bank.move_box((2, 4, 9), (5, 2, 21))
                bank.move_box((2, 4, 10), (5, 2, 22))
                bank.move_box((2, 4, 12), (5, 2, 23))
                bank.move_box((2, 4, 11), (5, 2, 24))
                day.emit(when, "AUDIT", f"F5/R2/B23  {bank.box_count((5, 2, 23))} tubes, labels read KESTREL; "
                                        f"inventory expects {bank.box_count((5, 2, 24))} other tubes there")
            day.plant(day.at(6, 15, 1), swap_audit)
            day.plant(day.at(6, 40, 1), lambda when: day.emit(
                when, "CORR", "EM-07: the box labels were swapped when the boxes were shelved at 02:28 and 02:30; "
                              "F5/R2/B23 holds the former F2/R4/B12 and F5/R2/B24 holds the former F2/R4/B11. "
                              "Inventory corrected; no tube changed position inside its box"))

    # ---------------------------------------------------------------- text

    def handover(self, day: Day) -> tuple[str, str]:
        rng = self.rng
        n = day.number
        lead, second = LEADS[n - 1]
        received = {
            1: "Quiet night. All six freezers between -78C and -81C. Nothing open.",
        }.get(n, f"See the handover at the end of day {n - 1:02d}; nothing else open.")
        s = day.stats
        given = [
            f"{s['RCV']} receipts, {s['MOVE']} moves, {s['PULL']} pulls (all returned), "
            f"{s['DISP']} disposals, {s['AUDIT']} audits."
        ]
        planted = {
            1: ("KESTREL intake done. Correction to the 10:09 note: Dr. Ivers phoned at 16:20, the \"REF\" "
                "sticker on S-0736 is the sender's mistake. The KESTREL reference aliquot is S-0731, the "
                "1.0 mL serum; S-0732, S-0733, S-0734 and S-0736 are working aliquots. The reference may "
                "only be thawed on her written request."),
            3: "KESTREL tubes from F2/R4/B12 consolidated into F3/R1/B05 under TR-2291, as Dr. Ivers asked.",
            4: "F4 defrost under way, its R1 boxes now live in F6/R1.",
            6: "TR-2291 voided after the morning audit (see 08:10 and 08:35).",
            7: "F2 service planned for 10-26, see the 11:00 PLAN note.",
            8: ("F2 failed at 02:10 and is off. Its R4 boxes are in F5/R2 (EM-07, mind the 06:40 "
                "correction). Next duty: confirm F5 holds below -78C."),
        }
        if n in planted:
            given.append(planted[n])
        given.append(rng.choice([
            "Freezers in range at the last round.",
            "Temperatures normal; LN2 tank above half.",
            "No alarms open at handover.",
        ]))
        return received, " ".join(given)

    def temps(self, day: Day, when: datetime) -> str:
        parts = []
        for f in FREEZERS:
            if f == 2 and day.number == DAYS and when >= day.at(2, 10, 1):
                parts.append("F2 off")
            else:
                parts.append(f"F{f} -{self.rng.randrange(780, 812) / 10:.1f}C")
        return " | ".join(parts)

    # ---------------------------------------------------------------- driver

    def run_day(self, number: int) -> None:
        rng = self.rng
        base = 2200 + 35 * (number - 1)
        pool = [t for t in range(base, base + 35) if t not in (2291, 2318)]
        day = Day(number, self.bank, rng, pool)
        self.planted_for(day)
        planted = sorted(day.planted, key=lambda item: item[0])

        when = day.start + timedelta(minutes=rng.randrange(3, 12))
        next_temp = day.start + timedelta(minutes=20)
        while when < day.end:
            while planted and planted[0][0] <= when:
                due, action = planted.pop(0)
                action(due)
            for item in sorted(p for p in self.pending if p[0] <= when):
                self.pending.remove(item)
                self.resolve(day, when, item[1], item[2])
            if when >= next_temp:
                day.emit(when, "TEMP", self.temps(day, when))
                next_temp += timedelta(minutes=rng.randrange(150, 210))
            else:
                self.noise_entry(day, when)
            when += timedelta(minutes=rng.randrange(14, 34))
        while planted:
            due, action = planted.pop(0)
            action(due)
        # Pulls never cross a day; voids and typos must be settled by the end.
        for item in sorted(p for p in self.pending if p[1] == "RET" or number == DAYS):
            self.pending.remove(item)
            self.resolve(day, min(item[0], day.end), item[1], item[2])

        lead, second = LEADS[number - 1]
        received, given = self.handover(day)
        end = day.start + timedelta(days=1)
        head = [
            f"BIOBANK CUSTODY LOG - duty day {number:02d} of {DAYS:02d}",
            f"Period: {day.start:%Y-%m-%d %H:%M} -> {end:%Y-%m-%d %H:%M}",
            f"Duty lead: {lead} / second: {second}",
            "Slot notation: freezer/rack/box/position, e.g. F1/R1/B01/A1 (box rows A-I, columns 1-9).",
            "",
            "-- Handover received --",
            received,
            "",
            "-- Entries --",
        ]
        tail = ["", "-- Handover to next duty --", given, ""]
        self.files[f"custody/day-{number:02d}.log"] = "\n".join(head + day.lines + tail)

    def build(self) -> dict[str, str]:
        for number in range(1, DAYS + 1):
            self.run_day(number)
        assert not self.pending, self.pending
        return self.files, self.bank


def build():
    return Log().build()


def self_check(files: dict[str, str], bank: Bank) -> None:
    text = "\n".join(files.values())
    assert bank.slot_of["S-0731"] == (5, 2, 23, "B7"), bank.slot_of["S-0731"]
    assert slot_str(bank.slot_of["S-0731"]) == ANSWER
    assert bank.slot_of["S-0713"] == (1, 3, 2, "D4")
    assert bank.slot_of["S-0736"] == (3, 2, 14, "E6")
    # The day-08 audit must tell the two boxes apart by their tube count.
    assert bank.box_count((5, 2, 23)) == 4 and bank.box_count((5, 2, 24)) != 4
    for sample in ("S-0137", "S-0173", "S-0317", "S-0371"):
        assert sample not in text, sample
    # Where each planted marker may appear (file names) -- grep's view.
    def where(needle: str) -> list[str]:
        return sorted(name for name, body in files.items() if needle in body)
    assert where("S-0731") == ["custody/day-01.log", "custody/day-03.log"], where("S-0731")
    assert where("TR-2291") == ["custody/day-03.log", "custody/day-06.log"], where("TR-2291")
    assert where("EM-07") == ["custody/day-08.log"]
    assert where("F5/R2/B23") == ["custody/day-08.log"]
    assert ANSWER not in text, "the answer must not be written anywhere"
    assert text.count("S-0731") == 3, text.count("S-0731")
    for body in files.values():
        assert body.isascii() and body.endswith("\n")


def write_tree(root: Path, files: dict[str, str]) -> None:
    for path, body in files.items():
        target = root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(body.encode())


def tree_bytes(root: Path) -> dict[str, bytes]:
    return {str(p.relative_to(root)): p.read_bytes() for p in sorted(root.rglob("*")) if p.is_file()}


def main() -> int:
    files, bank = build()
    self_check(files, bank)
    if "--check" in sys.argv[1:]:
        with tempfile.TemporaryDirectory() as tmp:
            write_tree(Path(tmp), files)
            want = tree_bytes(Path(tmp))
        got = tree_bytes(SETUP) if SETUP.is_dir() else {}
        if want != got:
            print("setup/ differs from the generator output", file=sys.stderr)
            return 1
        print("setup/ matches the generator", file=sys.stderr)
        return 0
    if SETUP.exists():
        shutil.rmtree(SETUP)
    write_tree(SETUP, files)
    sizes = {name: len(body.encode()) for name, body in files.items()}
    print(f"files: {len(files)}  bytes: {sum(sizes.values())}  " + " ".join(f"{v}" for v in sizes.values()))
    return 0


if __name__ == "__main__":
    sys.exit(main())
