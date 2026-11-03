import type { Key } from "@charet/i18n";
import { useQueryClient } from "@tanstack/react-query";
import {
  Check,
  CircleAlert,
  CircleCheck,
  Copy,
  ExternalLink,
  LoaderCircle,
  LogIn,
  LogOut,
} from "lucide-react";
import { type FormEvent, type ReactNode, useEffect, useRef, useState } from "react";
import { copyText } from "@/components/CopyOnce";
import { Section } from "@/components/Section";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/components/ui/segmented";
import type { KeyCheck, OAuthStatus, Provider, ProviderId } from "@/lib/api";
import { useT } from "@/lib/i18n";
import { useCheckKey, useProviderActions, useProviders, useSignInStatus } from "@/lib/queries";

const META: Record<ProviderId, { name: Key; agent: Key; soon?: true }> = {
  anthropic: { name: "web.providers.anthropic", agent: "web.providers.anthropic.agent" },
  openai: { name: "web.providers.openai", agent: "web.providers.openai.agent" },
  google: { name: "web.providers.google", agent: "web.providers.google.agent", soon: true },
};

const USING: Record<Provider["use"], Key> = {
  key: "web.providers.using.key",
  oauth: "web.providers.using.oauth",
  none: "web.providers.using.none",
};

const REASONS: Record<NonNullable<KeyCheck["reason"]>, Key> = {
  invalid: "web.providers.reason.invalid",
  forbidden: "web.providers.reason.forbidden",
  rate_limited: "web.providers.reason.rate_limited",
  unreachable: "web.providers.reason.unreachable",
};

/** The accounts the web agent runs on: an API key (checked before it is saved) or a sign-in. */
export function AgentProviders() {
  const t = useT();
  const providers = useProviders();
  return (
    <Section title={t("web.providers.title")} lead={t("web.providers.lead")}>
      {providers.error && <p className="p-3 text-sm text-destructive">{providers.error.message}</p>}
      {providers.data?.providers.map((provider) => (
        <ProviderBlock key={provider.id} provider={provider} />
      ))}
    </Section>
  );
}

function ProviderBlock({ provider }: { provider: Provider }) {
  const t = useT();
  const { choose } = useProviderActions();
  const meta = META[provider.id];
  const both = provider.key.source !== "none" && provider.oauth?.state === "signed_in";
  return (
    <section
      className="flex flex-col gap-3 p-4"
      aria-label={t(meta.name)}
      data-testid={`provider-${provider.id}`}
    >
      <div className="flex flex-wrap items-center gap-2">
        <div className="flex flex-col">
          <span className="font-medium">{t(meta.name)}</span>
          <span className="text-xs text-muted-foreground">{t(meta.agent)}</span>
        </div>
        <span className="flex-1" />
        {meta.soon && <Badge variant="outline">{t("web.providers.soon")}</Badge>}
        <Badge variant={provider.use === "none" ? "outline" : "info"}>
          {t(USING[provider.use])}
        </Badge>
      </div>
      <KeyRow provider={provider} />
      {provider.oauth && <SignInRow provider={provider} oauth={provider.oauth} />}
      {both && (
        <Row label={t("web.providers.use")}>
          <Segmented
            label={t("web.providers.use")}
            value={provider.use === "oauth" ? "oauth" : "key"}
            onChange={(use) => choose.mutate({ provider: provider.id, use })}
            options={[
              { value: "key", label: t("web.providers.useKey") },
              { value: "oauth", label: t("web.providers.useOauth") },
            ]}
          />
        </Row>
      )}
      {meta.soon && (
        <p className="text-xs text-muted-foreground">{t("web.providers.googleNote")}</p>
      )}
    </section>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5 sm:flex-row sm:items-start sm:gap-4">
      <span className="pt-2 text-sm text-muted-foreground sm:w-24 sm:shrink-0">{label}</span>
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">{children}</div>
    </div>
  );
}

/** Type a key, press Check: the provider says whether it works. Only a working key can be saved. */
function KeyRow({ provider }: { provider: Provider }) {
  const t = useT();
  const name = t(META[provider.id].name);
  const [key, setKey] = useState("");
  const check = useCheckKey();
  const { saveKey, removeKey } = useProviderActions();
  const typed = key.trim();
  const answer = check.data;
  const works = answer?.ok === true && check.variables?.key === typed;
  const env = provider.key.source === "env";
  const saved = provider.key.source === "settings";
  const last4 = provider.key.last4 ?? "";

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (typed && !check.isPending) check.mutate({ provider: provider.id, key: typed });
  };
  const save = () =>
    saveKey.mutate(
      { provider: provider.id, key: typed },
      {
        onSuccess: () => {
          setKey("");
          check.reset();
        },
      },
    );

  return (
    <Row label={t("web.providers.key")}>
      <form onSubmit={submit} className="flex flex-wrap items-center gap-2">
        <Input
          className="min-w-56 flex-1"
          type="password"
          autoComplete="off"
          value={key}
          disabled={env}
          onChange={(event) => {
            setKey(event.target.value);
            check.reset();
            saveKey.reset();
          }}
          placeholder={
            env
              ? t("web.providers.keyEnv", { last4 })
              : saved
                ? t("web.providers.keySaved", { last4 })
                : t("web.providers.keyNone")
          }
          aria-label={t("web.providers.keyFor", { provider: name })}
        />
        {env ? (
          <Badge variant="secondary">{t("web.providers.env")}</Badge>
        ) : (
          <>
            <Button
              type="submit"
              size="sm"
              variant="outline"
              disabled={!typed || check.isPending}
              data-testid={`check-${provider.id}`}
            >
              {check.isPending && <LoaderCircle className="animate-spin" />}
              {t(check.isPending ? "web.providers.checking" : "web.providers.check")}
            </Button>
            {works && (
              <Button
                type="button"
                size="sm"
                disabled={saveKey.isPending}
                onClick={save}
                data-testid={`save-${provider.id}`}
              >
                {t("web.providers.save")}
              </Button>
            )}
            {saved && !typed && (
              <Button
                type="button"
                size="sm"
                variant="ghost"
                disabled={removeKey.isPending}
                onClick={() => removeKey.mutate(provider.id)}
              >
                {t("web.providers.remove")}
              </Button>
            )}
          </>
        )}
      </form>
      <CheckResult
        answer={check.variables?.key === typed ? answer : undefined}
        error={check.error?.message ?? saveKey.error?.message}
      />
    </Row>
  );
}

function CheckResult({
  answer,
  error,
}: {
  answer: KeyCheck | undefined;
  error: string | undefined;
}) {
  const t = useT();
  if (error) return <Status bad>{error}</Status>;
  if (!answer) return null;
  if (answer.ok) {
    return (
      <Status>
        {answer.models === undefined
          ? t("web.providers.ok")
          : t("web.providers.okModels", { count: answer.models })}
      </Status>
    );
  }
  return <Status bad>{answer.reason ? t(REASONS[answer.reason]) : answer.message}</Status>;
}

function Status({ bad, children }: { bad?: boolean; children: ReactNode }) {
  const Icon = bad ? CircleAlert : CircleCheck;
  return (
    <p
      role={bad ? "alert" : "status"}
      className={`flex items-center gap-1.5 text-xs ${bad ? "text-destructive" : "text-success"}`}
    >
      <Icon className="size-3.5 shrink-0" />
      {children}
    </p>
  );
}

/**
 * Sign in with the provider's own login page, opened in a new window. What happens next depends on
 * the provider: ChatGPT shows a code to type at its page (this page notices when it is done);
 * Claude's page shows a code to paste back here. The server keeps the state, so the row looks the
 * same after a reload.
 */
function SignInRow({ provider, oauth }: { provider: Provider; oauth: OAuthStatus }) {
  const t = useT();
  const client = useQueryClient();
  const name = t(META[provider.id].name);
  const { startSignIn, signOut } = useProviderActions();
  const [problem, setProblem] = useState<string | undefined>();
  const popup = useRef<Window | null>(null);
  const waiting = oauth.state === "waiting";
  const polled = useSignInStatus(provider.id, waiting).data;

  // The server saw the sign-in end or fail: show the new state and close the window.
  useEffect(() => {
    if (!waiting || !polled || polled.state === "waiting") return;
    popup.current?.close();
    popup.current = null;
    void client.invalidateQueries({ queryKey: ["admin"] });
  }, [waiting, polled, client]);

  const open = () => {
    setProblem(undefined);
    // Browsers allow a window only inside the click, so it opens empty and gets its address next.
    const win = window.open("about:blank", "charet-sign-in", "popup,width=520,height=720");
    if (!win) {
      setProblem(t("web.providers.popupBlocked"));
      return;
    }
    popup.current = win;
    startSignIn.mutate(provider.id, {
      onSuccess: ({ url }) => {
        if (url) win.location.href = url;
      },
      onError: (error) => {
        win.close();
        setProblem(error.message);
      },
    });
  };
  // The sign-in under way keeps its code: the same page opens again, not a new sign-in.
  const reopen = () => {
    setProblem(undefined);
    if (!oauth.url) return;
    const win = window.open(oauth.url, "charet-sign-in", "popup,width=520,height=720");
    if (win) popup.current = win;
    else setProblem(t("web.providers.popupBlocked"));
  };
  const cancel = () => {
    popup.current?.close();
    popup.current = null;
    signOut.mutate(provider.id);
  };

  if (oauth.state === "signed_in") {
    return (
      <Row label={t("web.providers.signIn")}>
        <div className="flex flex-wrap items-center gap-2 text-sm">
          <CircleCheck className="size-4 text-success" />
          <span>
            {oauth.account
              ? t("web.providers.signedIn", { account: oauth.account })
              : t("web.providers.signedInPlain")}
          </span>
          <span className="flex-1" />
          <Button
            type="button"
            size="sm"
            variant="ghost"
            disabled={signOut.isPending}
            onClick={() => signOut.mutate(provider.id)}
          >
            <LogOut />
            {t("web.providers.signOut")}
          </Button>
        </div>
      </Row>
    );
  }

  return (
    <Row label={t("web.providers.signIn")}>
      {waiting ? (
        <Waiting provider={provider} oauth={oauth} onCancel={cancel} onReopen={reopen} />
      ) : (
        <div className="flex flex-wrap items-center gap-3">
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={startSignIn.isPending}
            onClick={open}
            data-testid={`sign-in-${provider.id}`}
          >
            <LogIn />
            {t(oauth.state === "failed" ? "web.providers.tryAgain" : "web.providers.signInWith", {
              provider: name,
            })}
          </Button>
          <span className="text-xs text-muted-foreground">{t("web.providers.signInLead")}</span>
        </div>
      )}
      {oauth.state === "failed" && !waiting && (
        <Status bad>{t("web.providers.failed", { error: oauth.error ?? "" })}</Status>
      )}
      {problem && <Status bad>{problem}</Status>}
    </Row>
  );
}

/** A sign-in in progress: the code to type (ChatGPT) or the box to paste the code into (Claude). */
function Waiting({
  provider,
  oauth,
  onCancel,
  onReopen,
}: {
  provider: Provider;
  oauth: OAuthStatus;
  onCancel: () => void;
  onReopen: () => void;
}) {
  const t = useT();
  const [copied, setCopied] = useState(false);
  const [code, setCode] = useState("");
  const { finishSignIn } = useProviderActions();
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (code.trim()) finishSignIn.mutate({ provider: provider.id, code: code.trim() });
  };
  return (
    <div className="flex flex-col gap-2.5" data-testid={`waiting-${provider.id}`}>
      {oauth.method === "device" && oauth.userCode ? (
        <div className="flex flex-col gap-2">
          <p className="text-sm">{t("web.providers.typeCode")}</p>
          <div className="flex flex-wrap items-center gap-2">
            <code
              className="rounded-md border bg-muted px-3 py-1.5 font-mono text-lg tracking-widest"
              data-testid={`user-code-${provider.id}`}
            >
              {oauth.userCode}
            </code>
            <Button
              type="button"
              size="sm"
              variant="outline"
              onClick={() => copyText(oauth.userCode ?? "").then((ok) => setCopied(ok))}
              onBlur={() => setCopied(false)}
            >
              {copied ? <Check /> : <Copy />}
              {t("web.providers.copyCode")}
            </Button>
          </div>
        </div>
      ) : (
        <form onSubmit={submit} className="flex flex-col gap-2">
          <p className="text-sm">{t("web.providers.pasteCode")}</p>
          <div className="flex flex-wrap items-center gap-2">
            <Input
              className="min-w-56 flex-1"
              autoComplete="off"
              value={code}
              onChange={(event) => {
                setCode(event.target.value);
                finishSignIn.reset();
              }}
              placeholder="code#state"
              aria-label={t("web.providers.codeFor", { provider: t(META[provider.id].name) })}
            />
            <Button
              type="submit"
              size="sm"
              disabled={!code.trim() || finishSignIn.isPending}
              data-testid={`submit-code-${provider.id}`}
            >
              {finishSignIn.isPending && <LoaderCircle className="animate-spin" />}
              {t("web.providers.submitCode")}
            </Button>
          </div>
          {finishSignIn.error && <Status bad>{finishSignIn.error.message}</Status>}
        </form>
      )}
      <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
        {oauth.method === "device" && (
          <>
            <LoaderCircle className="size-3.5 animate-spin" />
            <span>{t("web.providers.waiting")}</span>
          </>
        )}
        <span className="flex-1" />
        <Button type="button" size="sm" variant="ghost" onClick={onReopen}>
          <ExternalLink />
          {t("web.providers.reopen")}
        </Button>
        <Button type="button" size="sm" variant="ghost" onClick={onCancel}>
          {t("web.providers.cancel")}
        </Button>
      </div>
    </div>
  );
}
