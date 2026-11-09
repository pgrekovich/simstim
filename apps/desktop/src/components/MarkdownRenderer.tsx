import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

/** Whether an image source would be fetched from the network. */
function isRemote(src: string | undefined): boolean {
  return src !== undefined && /^(https?:)?\/\//i.test(src);
}

const markdownComponents: Components = {
  a({ node, ...props }) {
    void node;
    return <a {...props} target="_blank" rel="noreferrer" />;
  },
  // A remote image is a request made on every view, carrying the viewer's
  // address and time to a server a note, summary or chat answer named. It
  // is drawn as a link, so nothing leaves the Mac until the user clicks;
  // the window's Content Security Policy blocks such a load regardless.
  img({ node, src, alt, ...props }) {
    void node;
    if (isRemote(src)) {
      return (
        <a href={src} target="_blank" rel="noreferrer">
          {alt && alt.length > 0 ? alt : src}
        </a>
      );
    }
    return <img src={src} alt={alt} {...props} />;
  },
};

/**
 * Heavy parser kept in its own lazy chunk so it does not delay app startup.
 * `components` overrides individual elements (Open loops makes its task
 * items interactive); everything else renders as in notes and summaries.
 */
export default function MarkdownRenderer({
  source,
  components,
}: {
  source: string;
  components?: Components;
}) {
  return (
    <ReactMarkdown
      remarkPlugins={[remarkGfm]}
      components={components ? { ...markdownComponents, ...components } : markdownComponents}
    >
      {source}
    </ReactMarkdown>
  );
}
