/* named themes: project flavor over the same --ae-* token contract.

   Pin a project theme on the root:

   <html data-ae-theme="moss">

   To let users switch, inline this boot in <head> before first paint:

   <script>
     try {
       var t = localStorage.getItem('ae-theme');
       if (/^(ultramarine|moss|ember|violet)$/.test(t)) {
         document.documentElement.dataset.aeTheme = t;
       }
     } catch (e) {}
   </script>

   Then load this recipe and mark controls:

   <button data-ae-theme-choice="moss">moss</button>

   The recipe persists the user's selected theme, keeps aria-pressed in
   sync, and exposes window.aeTheme.set('violet') for app shells. */
(() => {
  if (window.aeTheme) return;

  const root = document.documentElement;
  const themes = ['ultramarine', 'moss', 'ember', 'violet'];
  const themeSet = new Set(themes);
  const storageKey = root.dataset.aeThemeKey || 'ae-theme';

  const clean = (value) =>
    typeof value === 'string' && themeSet.has(value) ? value : null;

  const current = () => clean(root.dataset.aeTheme) || 'ultramarine';

  const syncButtons = () => {
    const active = current();
    document.querySelectorAll('[data-ae-theme-choice]').forEach((button) => {
      const chosen = clean(button.getAttribute('data-ae-theme-choice'));
      if (!chosen) return;
      button.setAttribute('aria-pressed', String(chosen === active));
    });
  };

  const setTheme = (theme, options = {}) => {
    const chosen = clean(theme);
    if (!chosen) return current();
    root.dataset.aeTheme = chosen;
    if (options.persist !== false) {
      try {
        localStorage.setItem(storageKey, chosen);
      } catch (e) {}
    }
    syncButtons();
    return chosen;
  };

  const saved = (() => {
    try {
      return clean(localStorage.getItem(storageKey));
    } catch (e) {
      return null;
    }
  })();
  if (saved) setTheme(saved, { persist: false });
  else if (!clean(root.dataset.aeTheme)) root.dataset.aeTheme = 'ultramarine';

  document.querySelectorAll('[data-ae-theme-choice]').forEach((button) => {
    const theme = clean(button.getAttribute('data-ae-theme-choice'));
    if (!theme) return;
    button.setAttribute('type', button.getAttribute('type') || 'button');
    button.addEventListener('click', () => setTheme(theme));
  });
  syncButtons();

  window.aeTheme = {
    themes: [...themes],
    get: current,
    set: setTheme,
    clear() {
      try {
        localStorage.removeItem(storageKey);
      } catch (e) {}
      root.dataset.aeTheme = 'ultramarine';
      syncButtons();
      return current();
    },
  };
})();
