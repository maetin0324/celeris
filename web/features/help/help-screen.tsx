import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { DataList } from "../../components/ui/data-list";
import { Section } from "../../components/ui/panel";

const sections = [
  { id: "flow", title: "3 分で分かる流れ" },
  { id: "screens", title: "画面ごとの説明" },
  { id: "acceptance", title: "受け入れ条件" },
  { id: "status", title: "状態" },
  { id: "failure", title: "失敗したタスクの直し方" },
  { id: "mcp", title: "MCP で外から使う" },
] as const;

const linkClass = "inline-flex min-h-11 min-w-11 items-center text-primary underline hover:text-primary-hover";

/** 目次から飛べる h2 の節。区切り線と余白で分け、枠の入れ子を作らない。 */
function HelpSection({ id, title, children }: { id: string; title: string; children: ReactNode }) {
  return (
    <Section id={id} title={title} className="scroll-mt-6 border-t border-border pt-6">
      <div className="space-y-4 text-body text-foreground">{children}</div>
    </Section>
  );
}

/** 静的な案内。画面内で daemon の Query や loader を作らない。 */
export function HelpScreen() {
  return (
    <ScreenFrame title="ヘルプ" route="/help" description="Celeris で仕事を頼み、結果を受け取るまでの手引きです。">
      {/* 本文は日本語の読みやすい行長（token の container-prose-ja）に収める。 */}
      <div className="flex w-full max-w-prose-ja min-w-0 flex-col gap-6">
        <nav aria-label="ヘルプの目次" className="rounded-lg border border-border bg-surface p-3">
          <ul className="flex flex-wrap gap-2">
            {sections.map((section) => (
              <li key={section.id}>
                <a
                  className="inline-flex min-h-11 items-center rounded-md border border-border px-3 text-label text-primary underline hover:bg-accent"
                  href={`#${section.id}`}
                >
                  {section.title}
                </a>
              </li>
            ))}
          </ul>
        </nav>
        <HelpSection id="flow" title="3 分で分かる流れ">
          <ol className="list-decimal space-y-2 pl-5">
            <li>
              <Link className={linkClass} to="/tasks/new">
                タスクを作る
              </Link>
              。目的と受け入れ条件を書く。
            </li>
            <li>
              <Link className={linkClass} to="/inbox">
                受信箱
              </Link>
              で質問や承認待ちを確認する。
            </li>
            <li>タスク詳細で進行状況、run、成果物を確認する。</li>
          </ol>
        </HelpSection>
        <HelpSection id="screens" title="画面ごとの説明">
          <Section level={3} title="話しかける・判断する">
            <ul className="list-disc space-y-2 pl-5">
              <li>
                <Link className={linkClass} to="/">
                  Console
                </Link>
                では担当に話しかけられます。
              </li>
              <li>
                <Link className={linkClass} to="/reports">
                  報告
                </Link>
                では仕事の結果を読み、既読にできます。
              </li>
            </ul>
          </Section>
          <Section level={3} title="組織と案件を見る">
            <ul className="list-disc space-y-2 pl-5">
              <li>
                <Link className={linkClass} to="/org">
                  組織
                </Link>
                では担当の木を見て、課・部や skill を管理できます。
              </li>
              <li>
                <Link className={linkClass} to="/projects">
                  案件
                </Link>
                では目的と仕事の進行を確認できます。
              </li>
            </ul>
          </Section>
        </HelpSection>
        <HelpSection id="acceptance" title="受け入れ条件">
          <p>タスクの完了条件を先に書き、結果と証拠を見て受け入れます。承認が必要なときは受信箱に表示されます。</p>
        </HelpSection>
        <HelpSection id="status" title="状態">
          <Section level={3} title="進んでいる">
            <DataList
              items={[
                { label: "draft / ready", value: "作成直後・実行待ち" },
                { label: "running / reviewing", value: "実行中・結果の判定中" },
                { label: "blocked", value: "人への質問などで停止中。受信箱で答えると再開します" },
              ]}
            />
          </Section>
          <Section level={3} title="終わった">
            <DataList items={[{ label: "done / failed / cancelled", value: "完了・失敗・取消" }]} />
          </Section>
        </HelpSection>
        <HelpSection id="failure" title="失敗したタスクの直し方">
          <Section level={3} title="原因を見る">
            <p>タスク詳細の run ログとエラーを確認します。</p>
          </Section>
          <Section level={3} title="直して再開する">
            <p>原因を直した後、再試行または再レビューを行えます。</p>
          </Section>
        </HelpSection>
        <HelpSection id="mcp" title="MCP で外から使う">
          <p>
            MCP クライアントから Celeris
            のタスクと知識にアクセスできます。接続設定と認可はアカウント画面で確認してください。
          </p>
        </HelpSection>
      </div>
    </ScreenFrame>
  );
}
