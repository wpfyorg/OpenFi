import { useEffect, useState } from "preact/hooks";

/** Current location.hash, updated on the `hashchange` event; defaults to "#/". */
export function useHash(): string {
  const [hash, setHash] = useState(location.hash || "#/");
  useEffect(() => {
    const onChange = () => setHash(location.hash || "#/");
    window.addEventListener("hashchange", onChange);
    return () => window.removeEventListener("hashchange", onChange);
  }, []);
  return hash;
}
