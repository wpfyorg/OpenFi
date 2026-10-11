import { useEffect, useRef, useState } from "preact/hooks";
import { logout } from "../api";
import { useSnap } from "../context";
import { Icon } from "./icons";

/** Top bar: search (press "/" anywhere to focus), WAN status dot, and the user menu. */
export function Topbar({ onMenu }: { onMenu: () => void }) {
  const { snap, connected } = useSnap();
  const searchRef = useRef<HTMLInputElement>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key !== "/" || e.metaKey || e.ctrlKey || e.altKey) return;
      const t = e.target as HTMLElement;
      if (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable) return;
      e.preventDefault();
      searchRef.current?.focus();
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    if (!menuOpen) return;
    function onDoc(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setMenuOpen(false);
    }
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [menuOpen]);

  async function signOut() {
    setMenuOpen(false);
    try { await logout(); } catch { /* mock or expired session: proceed to login regardless */ }
    location.hash = "#/login";
  }

  const wan = snap?.wan;
  return (
    <header class="topbar">
      <button type="button" class="icon-btn mobile-only" onClick={onMenu} aria-label="Open menu">
        <Icon name="menu" size={18} />
      </button>
      <div class="search">
        <Icon name="search" size={14} color="var(--text-3)" />
        <input ref={searchRef} type="search" placeholder="Search devices, IPs, MACs, SSIDs…" aria-label="Search" />
        <kbd>/</kbd>
      </div>
      <div class="topbar-right">
        <span class="wan-status" title={wan?.ip ?? undefined}>
          <span class={`dot ${connected ? "ok" : "bad"}`} />
          <span class="mono">{wan?.ip ? `WAN ${wan.ip}` : connected ? "WAN" : "Disconnected"}</span>
        </span>
        <div class="user-menu" ref={menuRef}>
          <button type="button" class="user-btn" onClick={() => setMenuOpen((v) => !v)} aria-haspopup="menu" aria-expanded={menuOpen}>
            <Icon name="circle-user" size={17} />
            <span>root</span>
          </button>
          {menuOpen && (
            <div class="user-dropdown" role="menu">
              <a href="#/logs" role="menuitem" onClick={() => setMenuOpen(false)}>
                <Icon name="scroll-text" size={14} /> View logs
              </a>
              <button type="button" role="menuitem" onClick={signOut}>
                <Icon name="log-out" size={14} /> Sign out
              </button>
            </div>
          )}
        </div>
      </div>
    </header>
  );
}
