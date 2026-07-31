interface MarkdownNode {
  children?: MarkdownNode[];
  lang?: string | null;
  meta?: string | null;
  type?: string;
}

/**
 * Code Hike does not ship an ACL grammar. ACL uses HCL-compatible lexical
 * constructs, so highlight ACL fences with the HCL grammar while preserving
 * ACL as the language shown to readers.
 */
export function remarkAclSyntax() {
  return (tree: MarkdownNode) => {
    const visit = (node: MarkdownNode) => {
      if (node.type === 'code' && node.lang?.toLowerCase() === 'acl') {
        node.lang = 'hcl';
        node.meta = [node.meta, 'displayLanguage=ACL']
          .filter(Boolean)
          .join(' ');
      }

      node.children?.forEach(visit);
    };

    visit(tree);
  };
}
