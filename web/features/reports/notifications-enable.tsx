import { useState } from "react";
import { Button } from "../../components/ui/button";

export function NotificationsEnable() {
  const [permission, setPermission] = useState(() =>
    typeof Notification === "undefined" ? "unsupported" : Notification.permission,
  );
  if (permission === "unsupported") return <span>ブラウザ通知は利用できません。</span>;
  if (permission === "granted") return <span>ブラウザ通知は許可されています。</span>;
  if (permission === "denied") return <span>ブラウザ通知はブラウザで拒否されています。</span>;
  return (
    <Button onClick={async () => setPermission(await Notification.requestPermission())}>このサイトの通知を許可</Button>
  );
}
