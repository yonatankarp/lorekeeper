// A two-click button for what can't be undone (Remove in the Party dialog's players list). The first click (or Enter or
// Space) arms it: it says what the next click does, in a style of its own (button[data-armed] in settings.css). A second
// click within 5 seconds acts; waiting, or moving focus away, puts the button back.

/**
 * Makes `button` two-click: armed, it reads `armedLabel` and `say(prompt)` announces it (a live status line); the
 * second click runs `act()`. Disarming puts the button's own label back and says "".
 */
export function confirmClick(button, { armedLabel, prompt, act, say = () => {}, ms = 5000 }) {
  const label = button.textContent;
  let timer = null;
  const disarm = () => {
    if (timer === null) return;
    clearTimeout(timer);
    timer = null;
    delete button.dataset.armed;
    button.textContent = label;
    say("");
  };
  button.addEventListener("click", () => {
    if (timer !== null) {
      disarm();
      act();
      return;
    }
    button.dataset.armed = "true";
    button.textContent = armedLabel;
    button.focus(); // WebKit doesn't focus a clicked button, and only a focused one gets the blur that disarms it
    say(prompt);
    timer = setTimeout(disarm, ms);
  });
  button.addEventListener("blur", disarm);
}
