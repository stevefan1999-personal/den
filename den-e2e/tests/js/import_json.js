import { assertEquals } from "den:assert";
import note from "./fixtures/note.json" with { type: "json" };

assertEquals(note.title, "den");
assertEquals(note.n, 42);
