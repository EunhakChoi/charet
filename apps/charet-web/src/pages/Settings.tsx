import {
  Bot,
  Check,
  Copy,
  Globe,
  Link2,
  RefreshCw,
  Send,
  Trash2,
  Users,
  Webhook as WebhookIcon,
} from "lucide-react";
import { type FormEvent, useState } from "react";
import { Navigate, NavLink, useParams } from "react-router";
import { AgentCosts, AgentSettings } from "@/components/AgentAdmin";
import { AgentProviders } from "@/components/AgentProviders";
import { AppHeader } from "@/components/AppHeader";
import { CopyOnce, copyText } from "@/components/CopyOnce";
import { Section } from "@/components/Section";
import { SharingPanel } from "@/components/SharingPanel";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/components/ui/segmented";
import type { Member, Role, Webhook } from "@/lib/api";
import { useT } from "@/lib/i18n";
import {
  useAccess,
  useAdminActions,
  useMe,
  useProjects,
  useSystem,
  useUsers,
  useWebhookActions,
  useWebhooks,
} from "@/lib/queries";
import { cn } from "@/lib/utils";

const ROLES: Role[] = ["admin", "editor", "commenter", "viewer"];

const TABS = ["members", "access", "webhooks", "agent"] as const;
type Tab = (typeof TABS)[number];
const ICONS = { members: Users, access: Globe, webhooks: WebhookIcon, agent: Bot } as const;

/** `/settings/<tab>`: members, who may look and how it is shared, webhooks, the web agent (admins). */
export function Settings() {
  const t = useT();
  const me = useMe();
  const admin = me.data?.role === "admin";
  const tab = useParams().tab;
  if (!TABS.includes(tab as Tab)) return <Navigate to="/settings/members" replace />;
  const current = tab as Tab;
  return (
    <div className="flex min-h-svh flex-col">
      <AppHeader />
      <main className="mx-auto flex w-full max-w-6xl flex-1 flex-col gap-6 p-6">
        <h1 className="text-2xl font-semibold">{t("web.settings.title")}</h1>
        {admin ? (
          <div className="flex flex-col gap-6 md:flex-row md:gap-10">
            <nav
              aria-label={t("web.settings.sections")}
              className="flex shrink-0 gap-1 overflow-x-auto md:w-44 md:flex-col"
            >
              {TABS.map((id) => {
                const Icon = ICONS[id];
                return (
                  <NavLink
                    key={id}
                    to={`/settings/${id}`}
                    className={({ isActive }) =>
                      cn(
                        "flex items-center gap-2 rounded-md px-3 py-2 text-sm text-muted-foreground transition-colors hover:text-foreground",
                        isActive && "bg-muted font-medium text-foreground",
                      )
                    }
                  >
                    <Icon className="size-4" />
                    {t(`web.settings.tab.${id}`)}
                  </NavLink>
                );
              })}
            </nav>
            <div className="flex min-w-0 flex-1 flex-col gap-8">
              {current === "members" && <Members />}
              {current === "access" && (
                <>
                  <Visitors />
                  <SharingPanel />
                </>
              )}
              {current === "webhooks" && <Webhooks />}
              {current === "agent" && (
                <>
                  <AgentProviders />
                  <AgentSettings />
                  <AgentCosts />
                </>
              )}
            </div>
          </div>
        ) : (
          <p className="text-sm text-muted-foreground">{t("web.settings.adminsOnly")}</p>
        )}
      </main>
    </div>
  );
}

function Empty({ text }: { text: string }) {
  return <p className="p-4 text-sm text-muted-foreground">{text}</p>;
}

/** Everyone on the charet: add people, change their role and hand over their token. */
function Members() {
  const t = useT();
  const users = useUsers();
  const { add } = useAdminActions();
  const [name, setName] = useState("");
  const [role, setRole] = useState<Role>("editor");
  const [added, setAdded] = useState<string | undefined>();
  const all = users.data?.users ?? [];
  const submit = (event: FormEvent) => {
    event.preventDefault();
    add.mutate(
      { name: name.trim(), role },
      {
        onSuccess: (member) => {
          setName("");
          setAdded(member.id);
        },
      },
    );
  };
  return (
    <Section title={t("web.team.members")} count={all.length} lead={t("web.team.lead")}>
      <form onSubmit={submit} className="flex flex-wrap items-center gap-2 p-3">
        <Input
          className="w-48 flex-1"
          value={name}
          maxLength={60}
          onChange={(event) => setName(event.target.value)}
          placeholder={t("web.team.namePlaceholder")}
          aria-label={t("web.team.name")}
        />
        <Segmented
          label={t("web.team.role")}
          value={role}
          onChange={setRole}
          options={ROLES.map((r) => ({ value: r, label: t(`web.team.role.${r}`) }))}
        />
        <Button type="submit" disabled={add.isPending || !name.trim()}>
          {t("web.team.add")}
        </Button>
      </form>
      {add.error && <p className="px-3 pb-3 text-sm text-destructive">{add.error.message}</p>}
      {users.error && (
        <p className="p-3 text-sm text-destructive">
          {t("web.common.error", { message: users.error.message })}
        </p>
      )}
      {all.map((member) => (
        <MemberRow key={member.id} member={member} fresh={member.id === added} />
      ))}
    </Section>
  );
}

function MemberRow({ member, fresh }: { member: Member; fresh: boolean }) {
  const t = useT();
  const me = useMe();
  const system = useSystem();
  const { change, reissue, remove } = useAdminActions();
  const [name, setName] = useState(member.name);
  const self = me.data?.id === member.id;
  const home = system.data?.toolUrl ?? window.location.origin;
  const link = `${home}/?t=${encodeURIComponent(member.token)}`;
  const error = change.error ?? reissue.error ?? remove.error;
  const rename = () => {
    const next = name.trim();
    if (next && next !== member.name) change.mutate({ id: member.id, name: next });
    else setName(member.name);
  };
  return (
    <div className={`flex flex-col gap-2 p-3 ${fresh ? "bg-muted/50" : ""}`}>
      <div className="flex flex-wrap items-center gap-2">
        <Input
          className="h-8 w-44"
          value={name}
          maxLength={60}
          onChange={(event) => setName(event.target.value)}
          onBlur={rename}
          onKeyDown={(event) => {
            if (event.key === "Enter") event.currentTarget.blur();
          }}
          aria-label={t("web.team.rename", { name: member.name })}
        />
        {member.owner && <Badge variant="outline">{t("web.team.owner")}</Badge>}
        {self && <Badge variant="outline">{t("web.team.you")}</Badge>}
        <span className="text-xs text-muted-foreground">
          {member.lastUsedAt
            ? t("web.team.used", { when: new Date(member.lastUsedAt).toLocaleString() })
            : t("web.team.unused")}
        </span>
        <div className="ml-auto flex items-center gap-1">
          <Segmented
            label={t("web.team.role")}
            value={member.role}
            onChange={(role) => change.mutate({ id: member.id, role })}
            options={ROLES.map((role) => ({ value: role, label: t(`web.team.role.${role}`) }))}
          />
          {!member.owner && (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => remove.mutate(member.id)}
              aria-label={t("web.team.remove", { name: member.name })}
              title={t("web.team.remove", { name: member.name })}
            >
              <Trash2 />
            </Button>
          )}
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Input
          readOnly
          className="h-8 w-72 flex-1"
          value={member.token}
          aria-label={t("web.team.token", { name: member.name })}
          onFocus={(event) => event.target.select()}
        />
        <CopyButton value={member.token} label={t("web.team.copyToken")} icon={<Copy />} />
        <CopyButton value={link} label={t("web.team.copyLink")} icon={<Link2 />} />
        <Button
          size="sm"
          variant="ghost"
          onClick={() => reissue.mutate(member.id)}
          disabled={reissue.isPending}
          title={t("web.team.reissueHint")}
        >
          <RefreshCw />
          {t("web.team.reissue")}
        </Button>
      </div>
      {error && <p className="text-sm text-destructive">{error.message}</p>}
    </div>
  );
}

function CopyButton({
  value,
  label,
  icon,
}: {
  value: string;
  label: string;
  icon: React.ReactNode;
}) {
  const [copied, setCopied] = useState(false);
  return (
    <Button
      size="sm"
      variant="outline"
      onClick={() => copyText(value).then((ok) => setCopied(ok))}
      onBlur={() => setCopied(false)}
    >
      {copied ? <Check /> : icon}
      {label}
    </Button>
  );
}

/** What people without a token may do: nothing, look, or also comment (projects can differ). */
function Visitors() {
  const t = useT();
  const access = useAccess(true);
  const { visitors } = useAdminActions();
  const level = access.data?.visitors;
  return (
    <Section title={t("web.team.visitors")} lead={t("web.team.visitorsLead")}>
      <div className="flex flex-col gap-2 p-3">
        {level !== undefined && (
          <Segmented
            label={t("web.team.visitors")}
            value={level}
            onChange={(value) => visitors.mutate(value)}
            options={[
              { value: "none", label: t("web.team.visitorsOff") },
              { value: "view", label: t("web.team.visitorsOn") },
              { value: "comment", label: t("web.team.visitorsComment") },
            ]}
          />
        )}
        <p className="text-xs text-muted-foreground">{t("web.team.visitorsNote")}</p>
        {visitors.error && <p className="text-sm text-destructive">{visitors.error.message}</p>}
      </div>
    </Section>
  );
}

/** Addresses that get a signed POST when comments or versions happen. */
function Webhooks() {
  const t = useT();
  const hooks = useWebhooks();
  const projects = useProjects();
  const { create } = useWebhookActions();
  const [url, setUrl] = useState("");
  const [project, setProject] = useState("");
  const [events, setEvents] = useState<Set<string>>(new Set());
  const [secret, setSecret] = useState<{ id: string; value: string } | undefined>();
  const all = hooks.data?.events ?? [];
  const submit = (event: FormEvent) => {
    event.preventDefault();
    create.mutate(
      { url: url.trim(), events: [...events], project: project || null },
      {
        onSuccess: (created) => {
          setSecret({ id: created.webhook.id, value: created.secret });
          setUrl("");
          setEvents(new Set());
        },
      },
    );
  };
  const toggle = (name: string) => {
    const next = new Set(events);
    if (next.has(name)) next.delete(name);
    else next.add(name);
    setEvents(next);
  };
  return (
    <Section title={t("web.webhooks.title")}>
      <form onSubmit={submit} className="flex flex-col gap-3 p-3">
        <div className="flex gap-2">
          <Input
            value={url}
            onChange={(event) => setUrl(event.target.value)}
            placeholder="https://hooks.example.com/charet"
            aria-label={t("web.webhooks.url")}
          />
          <Button type="submit" disabled={create.isPending || !url.trim()}>
            {t("web.webhooks.add")}
          </Button>
        </div>
        <fieldset className="flex flex-wrap gap-x-4 gap-y-1 text-sm">
          <legend className="mb-1 text-xs text-muted-foreground">{t("web.webhooks.events")}</legend>
          {all.map((name) => (
            <label key={name} className="flex items-center gap-1.5">
              <input
                type="checkbox"
                checked={events.has(name)}
                onChange={() => toggle(name)}
                className="accent-primary"
              />
              <code className="text-xs">{name}</code>
            </label>
          ))}
        </fieldset>
        <label className="flex items-center gap-2 text-sm">
          <span className="text-xs text-muted-foreground">{t("web.webhooks.project")}</span>
          <select
            value={project}
            onChange={(event) => setProject(event.target.value)}
            className="rounded-md border bg-background px-2 py-1 text-sm"
          >
            <option value="">{t("web.webhooks.allProjects")}</option>
            {(projects.data?.projects ?? []).map((p) => (
              <option key={p.key} value={p.key}>
                {p.title}
              </option>
            ))}
          </select>
        </label>
        {create.error && <p className="text-sm text-destructive">{create.error.message}</p>}
      </form>
      {(hooks.data?.webhooks ?? []).map((hook) => (
        <WebhookRow
          key={hook.id}
          hook={hook}
          secret={secret?.id === hook.id ? secret.value : undefined}
          onSecret={(value) => setSecret({ id: hook.id, value })}
        />
      ))}
      {hooks.data?.webhooks.length === 0 && <Empty text={t("web.webhooks.empty")} />}
    </Section>
  );
}

function WebhookRow({
  hook,
  secret,
  onSecret,
}: {
  hook: Webhook;
  secret: string | undefined;
  onSecret: (secret: string) => void;
}) {
  const t = useT();
  const { remove, rotate, test } = useWebhookActions();
  const ok = hook.lastStatus !== null && hook.lastStatus >= 200 && hook.lastStatus < 300;
  return (
    <div className="flex flex-col gap-2 p-3">
      <div className="flex items-center gap-2">
        <span className="min-w-0 flex-1 truncate text-sm font-medium">{hook.url}</span>
        {hook.lastDeliveryAt && (
          <Badge variant={ok ? "secondary" : "destructive"}>
            {hook.lastStatus ?? t("web.webhooks.failed")}
          </Badge>
        )}
        <Button
          size="sm"
          variant="outline"
          onClick={() => test.mutate(hook.id)}
          disabled={test.isPending}
        >
          <Send />
          {t("web.webhooks.test")}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          onClick={() => rotate.mutate(hook.id, { onSuccess: (r) => onSecret(r.secret) })}
          aria-label={t("web.webhooks.rotate", { url: hook.url })}
          title={t("web.webhooks.rotate", { url: hook.url })}
        >
          <RefreshCw />
        </Button>
        <Button
          size="sm"
          variant="ghost"
          onClick={() => remove.mutate(hook.id)}
          aria-label={t("web.webhooks.remove", { url: hook.url })}
          title={t("web.webhooks.remove", { url: hook.url })}
        >
          <Trash2 />
        </Button>
      </div>
      <div className="text-xs text-muted-foreground">
        {hook.events.join(", ")}
        {hook.project ? ` · ${hook.project}` : ""} · {t("web.webhooks.secretEnds")} …
        {hook.secretLast4}
      </div>
      {hook.lastError && <p className="text-xs text-destructive">{hook.lastError}</p>}
      {secret && (
        <CopyOnce
          value={secret}
          label={t("web.webhooks.secret")}
          note={t("web.webhooks.secretOnce")}
        />
      )}
    </div>
  );
}
