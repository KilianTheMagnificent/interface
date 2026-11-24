import { isDevBuild } from "../product";

/** Product wordmark. On the dev channel, "Dev" is a canary gold chip. */
export function ProductName({ suffix = "" }: { readonly suffix?: string }) {
  return (
    <>
      Interface
      {isDevBuild() ? <span className="product-dev-badge">Dev</span> : null}
      {suffix}
    </>
  );
}
