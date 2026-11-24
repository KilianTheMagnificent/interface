import { describe, expect, it } from "vitest";
import {
  BROWSER_HOME_URL, browserAddress, browserHostNotFound, browserLoadError, browserPageUrl, browserSearchUrl,
  browserServerUrls, browserShortAddress, browserTabTitle,
} from "./browser-pane";

describe("Browser pane address bar", () => {
  it("reads a port or a local host as the dev server over http", () => {
    expect(browserAddress("5173")).toBe("http://localhost:5173/");
    expect(browserAddress(":3000")).toBe("http://localhost:3000/");
    expect(browserAddress("8765/admin?tab=2")).toBe("http://localhost:8765/admin?tab=2");
    expect(browserAddress("localhost:8080/admin?x=1")).toBe("http://localhost:8080/admin?x=1");
    expect(browserAddress("127.0.0.1:4000")).toBe("http://127.0.0.1:4000/");
    expect(browserAddress("[::1]:6000")).toBe("http://[::1]:6000/");
    expect(browserAddress("localhost")).toBe("http://localhost/");
  });

  it("reads a bare domain as https and keeps an explicit scheme", () => {
    expect(browserAddress("  example.com ")).toBe("https://example.com/");
    expect(browserAddress("docs.example.com:8443/guide#top")).toBe("https://docs.example.com:8443/guide#top");
    expect(browserAddress("http://example.com")).toBe("http://example.com/");
  });

  it("searches Google for anything that is not an address", () => {
    expect(browserAddress("how do I center a div"))
      .toBe("https://www.google.com/search?q=how%20do%20I%20center%20a%20div");
    expect(browserAddress("rust")).toBe("https://www.google.com/search?q=rust");
    expect(browserAddress("c++ & rust?")).toBe("https://www.google.com/search?q=c%2B%2B%20%26%20rust%3F");
    expect(browserAddress("99999")).toBe("https://www.google.com/search?q=99999");
    expect(browserAddress("example.com is down")).toBe("https://www.google.com/search?q=example.com%20is%20down");
    expect(browserSearchUrl("  node.js ")).toBe("https://www.google.com/search?q=node.js");
    expect(browserSearchUrl(" ")).toBeNull();
    expect(browserSearchUrl("a".repeat(8_200))).toBeNull();
  });

  it("loads only web pages, never another scheme or credentials", () => {
    for (const refused of ["", "   ", "file:///etc/hosts", "javascript:alert(1)", "about:blank",
      "mailto:a@example.com", "data:text/html,hi", "http://user:pass@example.com", "chrome://settings",
      "view-source:https://example.com"]) {
      expect(browserAddress(refused), refused).toBeNull();
    }
  });

  it("opens a new tab on Google", () => {
    expect(browserPageUrl(BROWSER_HOME_URL)).toBe("https://www.google.com/");
  });
});

describe("Browser pane servers", () => {
  it("lists each loopback listener once, as localhost, in the order given", () => {
    expect(browserServerUrls([
      [{ url: "http://127.0.0.1:5173" }, { url: "http://[::1]:5173" }],
      [{ url: "http://127.0.0.1:3000" }, { url: "not a url" }],
    ])).toEqual(["http://localhost:5173/", "http://localhost:3000/"]);
    expect(browserServerUrls([])).toEqual([]);
  });

  it("shows a local page by its port and anything else in full", () => {
    expect(browserShortAddress("http://localhost:5173/")).toBe("localhost:5173");
    expect(browserShortAddress("http://localhost:5173/app")).toBe("http://localhost:5173/app");
    expect(browserShortAddress("https://example.com/")).toBe("https://example.com/");
  });

  it("explains a failed load and ignores a cancelled one", () => {
    expect(browserLoadError(-3, "http://localhost:3000/")).toBeNull();
    expect(browserLoadError(-102, "http://localhost:3000/"))
      .toBe("Nothing is answering at localhost:3000. Start the dev server, then retry.");
    expect(browserLoadError(-102, "http://127.0.0.1:3000/app")).toContain("Start the dev server");
    expect(browserLoadError(-102, "https://example.com/")).toBe("https://example.com/ refused the connection.");
    expect(browserLoadError(-105, "https://nope.invalid/")).toContain("Could not find");
    expect(browserHostNotFound(-105) && browserHostNotFound(-137) && !browserHostNotFound(-102)).toBe(true);
    expect(browserLoadError(-202, "https://self-signed.example/")).toContain("certificate");
  });
});

describe("Browser tab labels", () => {
  it("names a tab by its page, else by where it is", () => {
    expect(browserTabTitle("Vite + React", "http://localhost:5173/")).toBe("Vite + React");
    expect(browserTabTitle("  ", "http://localhost:5173/app")).toBe("localhost:5173");
    expect(browserTabTitle(null, "https://docs.example.com/guide")).toBe("docs.example.com");
    expect(browserTabTitle(null, null)).toBe("Browser");
    expect(browserTabTitle("A\u0007 page", null)).toBe("A page");
  });

  it("restores only a web address", () => {
    expect(browserPageUrl("http://localhost:5173/app")).toBe("http://localhost:5173/app");
    expect(browserPageUrl("file:///etc/hosts")).toBeNull();
    expect(browserPageUrl(`https://example.com/${"a".repeat(8_200)}`)).toBeNull();
  });
});
