import React from "https://esm.sh/react@18.3.1";

export interface Note {
  id: number;
  body: string;
  created: string;
}

const CSS = `
:root { color-scheme: light dark; }
body {
  margin: 0;
  font-family: ui-sans-serif, system-ui, sans-serif;
  line-height: 1.5;
  background: Canvas;
  color: CanvasText;
}
main { max-width: 40rem; margin: 0 auto; padding: 2rem 1.25rem 4rem; }
h1 { font-size: 1.75rem; margin: 0 0 0.25rem; }
.lede { color: gray; margin: 0 0 1.5rem; }
form { display: grid; gap: 0.75rem; margin-bottom: 2rem; }
textarea, button {
  font: inherit;
  padding: 0.6rem 0.75rem;
  border: 1px solid gray;
  border-radius: 0.4rem;
  background: Canvas;
  color: CanvasText;
}
button { cursor: pointer; width: max-content; }
ul { list-style: none; padding: 0; margin: 0; display: grid; gap: 0.75rem; }
li {
  margin: 0;
  padding: 0.9rem 1rem;
  border: 1px solid gray;
  border-radius: 0.4rem;
}
li p { margin: 0 0 0.35rem; white-space: pre-wrap; }
time { font-size: 0.8rem; color: gray; }
.empty { color: gray; }
`;

export function Page(props: { notes: Note[] }): unknown {
  const notes = props.notes;
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>den notes</title>
        <style>{CSS}</style>
      </head>
      <body>
        <main>
          <h1>Notes</h1>
          <p className="lede">
            TSX rendered on the server, rows stored in SQLite. Ctrl+C stops den.
          </p>
          <form method="post" action="/notes">
            <textarea name="body" rows={4} required placeholder="Write a note"></textarea>
            <button type="submit">Save</button>
          </form>
          {notes.length === 0 ? (
            <p className="empty">No notes yet.</p>
          ) : (
            <ul>
              {notes.map((note) => (
                <li key={note.id}>
                  <p>{note.body}</p>
                  <time dateTime={note.created}>{note.created}</time>
                </li>
              ))}
            </ul>
          )}
        </main>
      </body>
    </html>
  );
}
