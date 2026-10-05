// Pure history logic, tested in node: the pages visited (Back / Forward) and the app's undoable actions.

/** Visited entries (page paths, null = Home) with a cursor, like a browser's history. */
export function navHistory() {
  let list = [];
  let at = -1;
  return {
    /** A newly opened entry: drops everything ahead of the cursor. Opening the entry you're on adds nothing. */
    visit(entry) {
      if (list[at] === entry) return;
      list = [...list.slice(0, at + 1), entry];
      at = list.length - 1;
    },
    /** Index of the nearest entry in direction `dir` (-1 back, 1 forward) that isn't `current` and passes `ok`, or -1. */
    find(dir, ok, current) {
      for (let i = at + dir; i >= 0 && i < list.length; i += dir) if (list[i] !== current && ok(list[i])) return i;
      return -1;
    },
    go(i) {
      at = i;
      return list[i];
    },
    entry: (i) => list[i],
  };
}

/**
 * Undo and redo stacks of { label, undo, redo } actions (async functions). A new action clears redo; the oldest
 * falls off past `cap`. An action whose undo or redo throws is dropped, so one stuck action can't block older ones.
 */
export function undoStack(cap = 50) {
  const done = [];
  const undone = [];
  const move = async (from, to, fn) => {
    const action = from.pop();
    if (!action) return null;
    await action[fn]();
    to.push(action);
    return action;
  };
  return {
    push(action) {
      done.push(action);
      if (done.length > cap) done.shift();
      undone.length = 0;
    },
    undo: () => move(done, undone, "undo"),
    redo: () => move(undone, done, "redo"),
    top: () => done.at(-1),
  };
}
