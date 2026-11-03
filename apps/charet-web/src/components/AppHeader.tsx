import { KeyRound, LogOut, Moon, Settings, Sun } from "lucide-react";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { Link } from "react-router";
import { TokenForm } from "@/components/AuthGate";
import { Inbox } from "@/components/Inbox";
import { Button } from "@/components/ui/button";
import { Segmented } from "@/components/ui/segmented";
import { useLocale, useT } from "@/lib/i18n";
import { useMe, useSignOut } from "@/lib/queries";
import { useUiTheme } from "@/lib/theme";
import { useVisitorName } from "@/lib/visitor";

export function AppHeader({ children, actions }: { children?: ReactNode; actions?: ReactNode }) {
  const t = useT();
  const { locale, setLocale } = useLocale();
  const { dark, toggle } = useUiTheme();
  const me = useMe();
  const signOut = useSignOut();
  const visitorName = useVisitorName();
  const who = me.data;
  return (
    <header className="flex h-12 shrink-0 items-center gap-3 border-b bg-background px-3">
      <Link
        to="/"
        className="flex items-center gap-2 font-semibold"
        aria-label={t("web.nav.projects")}
      >
        <img src="/favicon.svg" alt="" className="size-6" />
        <span>charet</span>
      </Link>
      <div className="flex min-w-0 flex-1 items-center gap-3">{children}</div>
      <div className="flex items-center gap-2">
        {actions}
        <Inbox />
        {who?.role === "admin" && (
          <Button variant="ghost" size="sm" asChild>
            <Link to="/settings">
              <Settings />
              {t("web.settings.nav")}
            </Link>
          </Button>
        )}
        <Segmented
          label={t("web.ui.language")}
          value={locale}
          onChange={setLocale}
          options={[
            { value: "en", label: "EN" },
            { value: "ko", label: "한" },
          ]}
        />
        <Button
          variant="ghost"
          size="icon-sm"
          onClick={toggle}
          aria-label={t("web.ui.dark")}
          aria-pressed={dark}
        >
          {dark ? <Sun /> : <Moon />}
        </Button>
        {who?.kind === "visitor" && (
          <>
            <span className="max-w-40 truncate text-sm text-muted-foreground">
              {visitorName(who.id ?? "")}
            </span>
            <EnterToken />
          </>
        )}
        {who?.kind === "user" && (
          <>
            <span className="max-w-40 truncate text-sm text-muted-foreground">{who.name}</span>
            <Button
              variant="ghost"
              size="icon-sm"
              onClick={() => signOut.mutate()}
              aria-label={t("web.auth.signOut")}
              title={t("web.auth.signOut")}
            >
              <LogOut />
            </Button>
          </>
        )}
      </div>
    </header>
  );
}

/** A visitor with a token enters it here and the page reloads as them. */
function EnterToken() {
  const t = useT();
  const [open, setOpen] = useState(false);
  const panel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (event: MouseEvent) => {
      if (!panel.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);
  return (
    <div className="relative" ref={panel}>
      <Button size="sm" variant="outline" onClick={() => setOpen(!open)} aria-expanded={open}>
        <KeyRound />
        {t("web.auth.enterToken")}
      </Button>
      {open && (
        <section
          aria-label={t("web.auth.enterToken")}
          className="absolute right-0 top-10 z-50 w-80 rounded-xl border bg-card p-4 text-card-foreground shadow-lg"
        >
          <TokenForm />
        </section>
      )}
    </div>
  );
}
