import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { SavedCredentialsPanel } from "./saved-credentials-panel";

it("shows policy and expiry with a delete action, without exposing credential material", () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(["browser", "saved-credentials"], {
    items: [
      {
        credential_id: "cred-private",
        policy_id: "manaba",
        created_at: 1_700_000_000,
        expires_at: 1_700_086_400,
        username: "SECRET-USER",
        password: "SECRET-PASSWORD",
      },
    ],
  });
  const html = renderToStaticMarkup(
    <QueryClientProvider client={client}>
      <SavedCredentialsPanel csrf="csrf" />
    </QueryClientProvider>,
  );
  expect(html).toContain("manaba");
  expect(html).toContain("期限:");
  expect(html).toContain("manaba の保存済みログイン情報を削除");
  expect(html).not.toContain("SECRET-");
  expect(html).not.toContain("cred-private");
});

it("explains when there is no saved credential", () => {
  const client = new QueryClient();
  client.setQueryData(["browser", "saved-credentials"], { items: [] });
  const html = renderToStaticMarkup(
    <QueryClientProvider client={client}>
      <SavedCredentialsPanel csrf="csrf" />
    </QueryClientProvider>,
  );
  expect(html).toContain("保存済みのログイン情報はありません。");
});
