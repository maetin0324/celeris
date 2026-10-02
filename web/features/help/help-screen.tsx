import { Link } from "@tanstack/react-router";
import { ScreenFrame } from "../../components/shell/screen-frame";

const sections = [
  { id: "flow", title: "3 分で分かる流れ" },
  { id: "screens", title: "画面ごとの説明" },
  { id: "acceptance", title: "受け入れ条件" },
  { id: "status", title: "状態" },
  { id: "failure", title: "失敗したタスクの直し方" },
  { id: "mcp", title: "MCP で外から使う" },
] as const;

function Section({ id, title, children }: { id: string; title: string; children: React.ReactNode }) {
  return (
    <section id={id} className="scroll-mt-6 space-y-2 rounded border p-4" aria-labelledby={`${id}-title`}>
      <h2 id={`${id}-title`} className="text-lg font-semibold">
        {title}
      </h2>
      {children}
    </section>
  );
}

/** 静的な案内。画面内で daemon の Query や loader を作らない。 */
export function HelpScreen() {
  return (
    <ScreenFrame title="ヘルプ" route="/help">
      <nav aria-label="ヘルプの目次" className="rounded border p-3">
        <ul className="flex flex-wrap gap-2">
          {sections.map((section) => (
            <li key={section.id}>
              <a className="inline-flex min-h-11 items-center rounded border px-3 underline" href={`#${section.id}`}>
                {section.title}
              </a>
            </li>
          ))}
        </ul>
      </nav>
      <Section id="flow" title="3 分で分かる流れ">
        <ol className="list-decimal space-y-2 pl-5">
          <li>
            <Link className="inline-flex min-h-11 min-w-11 items-center underline" to="/tasks/new">
              タスクを作る
            </Link>
            。目的と受け入れ条件を書く。
          </li>
          <li>
            <Link className="inline-flex min-h-11 min-w-11 items-center underline" to="/inbox">
              受信箱
            </Link>
            で質問や承認待ちを確認する。
          </li>
          <li>タスク詳細で進行状況、run、成果物を確認する。</li>
        </ol>
      </Section>
      <Section id="screens" title="画面ごとの説明">
        <ul className="list-disc space-y-2 pl-5">
          <li>
            <Link className="inline-flex min-h-11 min-w-11 items-center underline" to="/">
              Console
            </Link>
            では担当に話しかけられます。
          </li>
          <li>
            <Link className="inline-flex min-h-11 min-w-11 items-center underline" to="/org">
              組織
            </Link>
            では担当の木を見て、課・部や skill を管理できます。
          </li>
          <li>
            <Link className="inline-flex min-h-11 min-w-11 items-center underline" to="/projects">
              案件
            </Link>
            では目的と仕事の進行を確認できます。
          </li>
          <li>
            <Link className="inline-flex min-h-11 min-w-11 items-center underline" to="/reports">
              報告
            </Link>
            では仕事の結果を読み、既読にできます。
          </li>
        </ul>
      </Section>
      <Section id="acceptance" title="受け入れ条件">
        <p>タスクの完了条件を先に書き、結果と証拠を見て受け入れます。承認が必要なときは受信箱に表示されます。</p>
      </Section>
      <Section id="status" title="状態">
        <dl className="grid gap-2 sm:grid-cols-2">
          <div>
            <dt className="font-medium">draft / ready</dt>
            <dd>作成直後・実行待ち</dd>
          </div>
          <div>
            <dt className="font-medium">running / reviewing</dt>
            <dd>実行中・結果の判定中</dd>
          </div>
          <div>
            <dt className="font-medium">blocked</dt>
            <dd>人への質問などで停止中</dd>
          </div>
          <div>
            <dt className="font-medium">done / failed / cancelled</dt>
            <dd>完了・失敗・取消</dd>
          </div>
        </dl>
      </Section>
      <Section id="failure" title="失敗したタスクの直し方">
        <p>タスク詳細の run ログとエラーを確認します。原因を直した後、再試行または再レビューを行えます。</p>
      </Section>
      <Section id="mcp" title="MCP で外から使う">
        <p>
          MCP クライアントから Celeris
          のタスクと知識にアクセスできます。接続設定と認可はアカウント画面で確認してください。
        </p>
      </Section>
    </ScreenFrame>
  );
}
