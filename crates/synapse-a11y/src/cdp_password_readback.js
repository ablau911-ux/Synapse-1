function passwordReadback(element, kind = "value", fallback = null) {
  const marker = "[REDACTED]";
  // The page's one copy of the rule of credential_redaction.rs (#2339).
  const CREDENTIAL_NAMES = [
    "token", "secret", "password", "passwd", "pwd", "cookie", "authorization", "auth", "bearer",
    "api key", "apikey", "session", "sessionid", "csrf", "cvv", "cvc", "pin", "otp", "card number",
    "cardnumber", "cc number", "ccnumber", "credential", "private key", "privatekey", "passcode",
    "one time code", "one timecode", "onetime code", "onetimecode", "recovery code", "recoverycode",
  ].map(name => name.split(" "));
  const TOKEN_PREFIXES = [
    "sk-", "ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_", "bearer ", "xoxb-", "xoxp-",
    "synapse-activation-v1-",
  ];
  const TOKEN_PREFIX_MIN_TAIL = 8;
  const OPAQUE_RUN_MIN_LEN = 16;
  const tagName = node => String(node?.tagName || "").toLowerCase();
  const isPassword = node => tagName(node) === "input" &&
    String(node.getAttribute?.("type") || node.type || "").toLowerCase() === "password";
  const matchesCredentialParts = name => {
    const parts = name.toLowerCase().split(/[^a-z]+/).filter(Boolean);
    return parts.some((_, start) =>
      CREDENTIAL_NAMES.some(words => words.every((part, offset) => parts[start + offset] === part)));
  };
  const credentialName = name => {
    const text = String(name ?? "");
    const splitName = text
      .replace(/([A-Z]+)([A-Z][a-z])/g, "$1 $2")
      .replace(/([a-z])([A-Z])/g, "$1 $2");
    return matchesCredentialParts(splitName) || matchesCredentialParts(text);
  };
  // Same rule as token_like_value in credential_redaction.rs.
  const tokenLike = value => {
    const text = String(value ?? "").trim().replace(/^["'\x60]+|["'\x60]+$/g, "");
    const knownPrefix = candidate => TOKEN_PREFIXES.some(prefix =>
      candidate.length >= prefix.length + TOKEN_PREFIX_MIN_TAIL && candidate.toLowerCase().startsWith(prefix));
    const opaqueRun = run => run.length >= OPAQUE_RUN_MIN_LEN && /[A-Za-z]/.test(run) && /[0-9]/.test(run);
    if (knownPrefix(text)) return true;
    return text.split(/\s+/).some(word => {
      const token = word.replace(/^["'()[\]{}<>,;]+|["'()[\]{}<>,;]+$/g, "");
      return knownPrefix(token) || token.split(/[^A-Za-z0-9]+/).some(opaqueRun);
    });
  };
  // One node's own facts: its type, autocomplete, and the names and labels a person reads.
  const sensitiveNode = node => {
    const attr = key => String(node?.getAttribute?.(key) || "");
    const type = attr("type").toLowerCase();
    const autocomplete = attr("autocomplete").toLowerCase();
    if (type === "password" || type === "hidden" ||
        /(?:password|one-time-code)/.test(autocomplete) ||
        autocomplete.split(/\s+/).some(part => part.startsWith("cc-"))) {
      return true;
    }
    const namedLabels = attr("aria-labelledby").split(/\s+/).filter(Boolean).map(id =>
      (node.getRootNode?.().getElementById?.(id) || document.getElementById(id))?.textContent || "");
    const controlLabels = Array.from(node?.labels || [], label => label.textContent || "");
    const names = [
      attr("aria-label"), attr("name"), attr("id"), attr("placeholder"),
      ...namedLabels, ...controlLabels
    ];
    return names.some(credentialName);
  };
  const isSecret = node => isPassword(node) || (tagName(node) === "input" &&
    (sensitiveNode(node) || tokenLike(node.value) || tokenLike(node.getAttribute?.("value"))));
  const isScript = node => tagName(node) === "script";
  if (kind === "is_password") return isPassword(element);
  if (kind === "sensitive_node") return sensitiveNode(element);
  if (kind === "credential_name") return credentialName(fallback);
  if (kind === "token_like") return tokenLike(fallback);
  const privateValue = isSecret(element);
  if (kind === "attributes") {
    const attributes = Object.fromEntries(Array.from(element?.attributes || [],
      attr => [attr.name, privateValue && attr.name.toLowerCase() === "value" ? marker : String(attr.value ?? "")]));
    if (privateValue) attributes.value = marker;
    return attributes;
  }
  // A copy of the element in an inert document, safe to serialize. Secrecy is decided on
  // the LIVE node (a copy has lost its link to its <label>) and the verdict is carried to
  // its copy: the two trees are walked in step. Script text is masked on the copy.
  const maskedCopy = () => {
    const inert = document.implementation.createHTMLDocument("");
    const clone = inert.importNode(element, true);
    const pending = [[element, clone]];
    while (pending.length) {
      const [live, copy] = pending.pop();
      if (isSecret(live)) {
        copy.setAttribute("value", marker);
        copy.value = marker;
      }
      if (isScript(copy) && copy.textContent) copy.textContent = marker;
      const liveChildren = Array.from(live.children || []);
      Array.from(copy.children || []).forEach((child, index) => pending.push([liveChildren[index] || child, child]));
      if (live.content && copy.content) pending.push([live.content, copy.content]);
    }
    return clone;
  };
  if (kind === "outerHTML" || kind === "innerHTML") {
    if (!element) return "";
    return String(maskedCopy()[kind] || "");
  }
  if (kind === "text" && isScript(element)) return marker;
  // The text of an element holding script text is read from the masked copy.
  const holdsScriptText = () => Array.from(element?.querySelectorAll?.("script") || []).some(script => script.textContent);
  if (kind === "text" && !privateValue && holdsScriptText()) return String(maskedCopy().textContent || "");
  return privateValue ? marker : fallback;
}
