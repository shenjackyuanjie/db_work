(() => {
  const shell = window.parent !== window ? window.parent.AppShell : null;
  if (!shell || !location.pathname.startsWith("/app-content/")) return;

  const style = document.createElement("style");
  style.textContent = `
    body > .app-header { display: none !important; }
    body { min-height: 100vh; }
    .sidebar { top: 0; }
    .sidebar > .brand, .sidebar > .switch { display: none; }
    .sidebar > nav { margin-top: 0; }
    .orchard-header { display: none !important; }
  `;
  document.head.append(style);

  document.addEventListener(
    "click",
    (event) => {
      const link = event.target.closest("a[href]");
      if (
        !link ||
        event.button !== 0 ||
        event.ctrlKey ||
        event.metaKey ||
        event.shiftKey ||
        event.altKey ||
        link.hasAttribute("download") ||
        link.target === "_blank"
      )
        return;
      if (link.hasAttribute("data-admin-section")) return;
      const url = shell.normalize(link.href);
      if (!url) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      void shell.navigate(url.href);
    },
    true,
  );

  // Keep in-page admin navigation inside this document while exposing its state
  // in the application URL, so bookmarks and browser navigation remain useful.
  const nativePush = history.pushState.bind(history);
  history.pushState = (state, unused, value) => {
    const url = value == null ? null : shell.normalize(value);
    if (!url) return nativePush(state, unused, value);
    history.replaceState(
      state,
      unused,
      `/app-content/admin${url.search}${url.hash}`,
    );
    shell.sync(url.href, { push: true });
  };
})();
