import { createFileRoute } from "@tanstack/react-router";
import { AccountsScreen } from "../features/ops/accounts-screen";

// R31 /accounts。loader は置かず fetch を待たない。
export const Route = createFileRoute("/accounts")({ component: AccountsScreen });
