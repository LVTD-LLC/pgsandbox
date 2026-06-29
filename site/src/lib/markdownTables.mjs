function isElement(node, tagName) {
  return node?.type === 'element' && node.tagName === tagName;
}

function findFirstElement(node, tagName) {
  if (isElement(node, tagName)) {
    return node;
  }

  for (const child of node.children || []) {
    const match = findFirstElement(child, tagName);

    if (match) {
      return match;
    }
  }

  return undefined;
}

function tableColumnCount(table) {
  const firstRow = findFirstElement(table, 'tr');

  return (firstRow?.children || []).filter((child) => isElement(child, 'th') || isElement(child, 'td')).length;
}

function wrapMarkdownTables(node) {
  if (!Array.isArray(node.children)) {
    return;
  }

  node.children = node.children.map((child) => {
    wrapMarkdownTables(child);

    if (!isElement(child, 'table')) {
      return child;
    }

    const frameClasses = ['markdown-table-frame'];

    if (tableColumnCount(child) >= 3) {
      frameClasses.push('markdown-table-frame-wide');
    }

    return {
      type: 'element',
      tagName: 'figure',
      properties: { className: frameClasses },
      children: [
        {
          type: 'element',
          tagName: 'div',
          properties: {
            className: ['markdown-table-scroll'],
            role: 'region',
            ariaLabel: 'Scrollable table',
            tabIndex: 0
          },
          children: [child]
        }
      ]
    };
  });
}

export function rehypeResponsiveTables() {
  return function transform(tree) {
    wrapMarkdownTables(tree);
  };
}
