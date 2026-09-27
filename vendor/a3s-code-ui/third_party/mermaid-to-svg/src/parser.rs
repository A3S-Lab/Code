use std::collections::{HashMap, HashSet};

use crate::ast::{
    Edge, EdgeStyle, FlowchartGraph, GraphDirection, Node, NodeShape, Statement, StyleStatement,
    Subgraph,
};
use crate::error::MermaidError;

pub fn parse_mermaid(input: &str) -> Result<FlowchartGraph, MermaidError> {
    if let Some(first_line) = first_non_empty_non_comment_line(input) {
        let first_token = first_line.split_whitespace().next().unwrap_or("");

        if first_token != "graph"
            && first_token != "flowchart"
            && is_known_mermaid_type(first_token)
        {
            return Err(MermaidError::UnsupportedDiagramType(
                first_token.to_string(),
            ));
        }
    }

    let mut parser = Parser::new(input);
    parser.parse()
}

fn normalize_label(label: &str) -> String {
    let label = strip_wrapping_quotes(label.trim());
    decode_html_entities(label)
        .replace("\\n", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("<br>", "\n")
        .replace("<BR/>", "\n")
        .replace("<BR />", "\n")
        .replace("<BR>", "\n")
}

fn decode_html_entities(label: &str) -> String {
    label
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn strip_wrapping_quotes(label: &str) -> &str {
    let bytes = label.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        &label[1..label.len() - 1]
    } else {
        label
    }
}

fn first_non_empty_non_comment_line(input: &str) -> Option<&str> {
    input
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && !l.starts_with("%%"))
}

fn is_known_mermaid_type(token: &str) -> bool {
    matches!(
        token,
        "sequenceDiagram"
            | "classDiagram"
            | "classDiagram-v2"
            | "stateDiagram"
            | "stateDiagram-v2"
            | "erDiagram"
            | "journey"
            | "gantt"
            | "pie"
            | "mindmap"
            | "timeline"
            | "info"
            | "kanban"
            | "gitGraph"
            | "requirementDiagram"
            | "C4Context"
            | "C4Container"
            | "C4Component"
            | "C4Dynamic"
            | "C4Deployment"
            | "sankey-beta"
            | "packet-beta"
            | "xychart-beta"
            | "radar-beta"
            | "block-beta"
            | "flowchart-elk"
            | "quadrantChart"
    )
}

struct Parser<'a> {
    lines: Vec<&'a str>,
    current_line: usize,
    next_subgraph_index: usize,
    class_defs: HashMap<String, Vec<(String, String)>>,
    node_classes: HashMap<String, Vec<String>>,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        let lines: Vec<&str> = input.lines().collect();
        Self {
            lines,
            current_line: 0,
            next_subgraph_index: 0,
            class_defs: HashMap::new(),
            node_classes: HashMap::new(),
        }
    }

    fn parse(&mut self) -> Result<FlowchartGraph, MermaidError> {
        let direction = self.parse_graph_declaration()?;
        let mut statements = self.parse_statements()?;
        self.apply_class_styles(&mut statements);

        Ok(FlowchartGraph {
            direction,
            statements,
        })
    }

    fn current_line_content(&self) -> Option<&'a str> {
        self.lines.get(self.current_line).map(|s| s.trim())
    }

    fn advance(&mut self) {
        self.current_line += 1;
    }

    fn skip_empty_lines(&mut self) {
        while let Some(line) = self.current_line_content() {
            if line.is_empty() || line.starts_with("%%") {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn parse_graph_declaration(&mut self) -> Result<GraphDirection, MermaidError> {
        self.skip_empty_lines();

        let line = self
            .current_line_content()
            .ok_or_else(|| MermaidError::ParseError {
                line: self.current_line + 1,
                message: "Expected graph declaration".to_string(),
            })?;

        let direction = if line.starts_with("graph ") || line.starts_with("flowchart ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                return Err(MermaidError::ParseError {
                    line: self.current_line + 1,
                    message: "Expected direction after 'graph' or 'flowchart'".to_string(),
                });
            }
            self.parse_direction(parts[1])?
        } else {
            return Err(MermaidError::ParseError {
                line: self.current_line + 1,
                message: "Expected 'graph' or 'flowchart' declaration".to_string(),
            });
        };

        self.advance();
        Ok(direction)
    }

    fn parse_direction(&self, dir: &str) -> Result<GraphDirection, MermaidError> {
        match dir.to_uppercase().as_str() {
            "TD" | "TB" => Ok(GraphDirection::TopToBottom),
            "BT" => Ok(GraphDirection::BottomToTop),
            "LR" => Ok(GraphDirection::LeftToRight),
            "RL" => Ok(GraphDirection::RightToLeft),
            _ => Err(MermaidError::InvalidDirection(dir.to_string())),
        }
    }

    fn parse_statements(&mut self) -> Result<Vec<Statement>, MermaidError> {
        let mut statements = Vec::new();

        while self.current_line_content().is_some() {
            self.skip_empty_lines();

            let Some(line) = self.current_line_content() else {
                break;
            };

            if line.is_empty() {
                self.advance();
                continue;
            }

            if line == "end" {
                break;
            }

            if line.starts_with("subgraph ") {
                statements.push(Statement::Subgraph(self.parse_subgraph()?));
            } else if line.starts_with("style ") {
                statements.push(Statement::Style(self.parse_style()?));
            } else if line.starts_with("classDef ") {
                self.parse_class_def();
            } else if is_ignored_flowchart_directive(line) {
                self.advance();
            } else if self.line_contains_edge(line) {
                let edge_statements = self.parse_edge_chain(line)?;
                statements.extend(edge_statements);
                self.advance();
            } else {
                for (_, node) in self.parse_node_group_str(line) {
                    if let Some(node) = node {
                        statements.push(Statement::Node(node));
                    }
                }
                self.advance();
            }
        }

        Ok(statements)
    }

    fn line_contains_edge(&self, line: &str) -> bool {
        self.find_edge_start(line).is_some()
    }

    fn parse_edge_chain(&mut self, line: &str) -> Result<Vec<Statement>, MermaidError> {
        let mut edges = Vec::new();
        let mut all_nodes: Vec<(String, Option<Node>)> = Vec::new();
        let mut remaining = line.trim();

        let first_node_end = self.find_edge_start(remaining).unwrap_or(remaining.len());
        let mut prev_group = self.parse_node_group_str(&remaining[..first_node_end]);
        all_nodes.extend(prev_group.iter().cloned());
        remaining = remaining[first_node_end..].trim_start();

        while !remaining.is_empty() {
            let (edge_style, label, edge_len) = self.parse_edge_syntax(remaining)?;
            remaining = remaining[edge_len..].trim_start();

            let next_node_end = self.find_edge_start(remaining).unwrap_or(remaining.len());
            let next_node_str = remaining[..next_node_end].trim();

            if next_node_str.is_empty() {
                break;
            }

            let next_group = self.parse_node_group_str(next_node_str);
            for (from_id, _) in &prev_group {
                for (to_id, _) in &next_group {
                    edges.push(Statement::Edge(Edge {
                        from: from_id.clone(),
                        to: to_id.clone(),
                        label: label.clone(),
                        style: edge_style,
                    }));
                }
            }

            all_nodes.extend(next_group.iter().cloned());
            prev_group = next_group;
            remaining = remaining[next_node_end..].trim_start();
        }

        let mut statements: Vec<Statement> = all_nodes
            .into_iter()
            .filter_map(|(_, node_opt)| node_opt.map(Statement::Node))
            .collect();
        statements.append(&mut edges);
        Ok(statements)
    }

    fn parse_node_group_str(&mut self, s: &str) -> Vec<(String, Option<Node>)> {
        split_ampersand_group(s)
            .into_iter()
            .map(|part| self.parse_one_node(part))
            .collect()
    }

    fn parse_one_node(&mut self, raw: &str) -> (String, Option<Node>) {
        let (body, classes) = split_class_annotation(raw);
        let node = self.try_parse_node(body);
        let id = match &node {
            Some(n) => n.id.clone(),
            None => self.extract_node_id(body),
        };
        if !classes.is_empty() && !id.is_empty() {
            self.node_classes.insert(id.clone(), classes);
        }
        (id, node)
    }

    /// Byte index where the first edge token starts, ignoring tokens inside
    /// bracket/quote-delimited node labels (`[..]`, `(..)`, `{..}`, `".."`).
    fn find_edge_start(&self, s: &str) -> Option<usize> {
        const PATTERNS: [&str; 9] = ["-.->", "-.-", "-->", "---", "==>", "===", "--", "==", "-."];
        let bytes = s.as_bytes();
        let mut depth: usize = 0;
        let mut in_quote = false;
        for i in 0..bytes.len() {
            let b = bytes[i];
            if in_quote {
                if b == b'"' {
                    in_quote = false;
                }
                continue;
            }
            match b {
                b'"' => in_quote = true,
                b'[' | b'(' | b'{' => depth += 1,
                b']' | b')' | b'}' => depth = depth.saturating_sub(1),
                _ if depth == 0 => {
                    if PATTERNS
                        .iter()
                        .any(|p| bytes[i..].starts_with(p.as_bytes()))
                    {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn parse_edge_syntax(
        &self,
        s: &str,
    ) -> Result<(EdgeStyle, Option<String>, usize), MermaidError> {
        let s = s.trim_start();

        let edge_patterns: &[(&str, EdgeStyle, &str)] = &[
            ("-->|", EdgeStyle::Arrow, "|"),
            ("---|", EdgeStyle::Line, "|"),
            ("-.->|", EdgeStyle::DottedArrow, "|"),
            ("-.-|", EdgeStyle::DottedLine, "|"),
            ("==>|", EdgeStyle::ThickArrow, "|"),
            ("===|", EdgeStyle::ThickLine, "|"),
            ("-->", EdgeStyle::Arrow, ""),
            ("---", EdgeStyle::Line, ""),
            ("-.->", EdgeStyle::DottedArrow, ""),
            ("-.-", EdgeStyle::DottedLine, ""),
            ("==>", EdgeStyle::ThickArrow, ""),
            ("===", EdgeStyle::ThickLine, ""),
        ];

        for (pattern, style, label_end) in edge_patterns {
            if let Some(after_pattern) = s.strip_prefix(pattern) {
                if !label_end.is_empty() {
                    if let Some(end_idx) = after_pattern.find(label_end) {
                        let label = normalize_label(&after_pattern[..end_idx]);
                        let total_len = pattern.len() + end_idx + label_end.len();
                        return Ok((*style, Some(label), total_len));
                    }
                } else {
                    return Ok((*style, None, pattern.len()));
                }
            }
        }

        // Open-label forms: `-- text -->`, `-- text ---`, `== text ==>`,
        // `== text ===`, `-. text .->`, `-. text .-`.
        let open_patterns: &[(&str, &[(&str, EdgeStyle)])] = &[
            ("--", &[("-->", EdgeStyle::Arrow), ("---", EdgeStyle::Line)]),
            (
                "==",
                &[
                    ("==>", EdgeStyle::ThickArrow),
                    ("===", EdgeStyle::ThickLine),
                ],
            ),
            (
                "-.",
                &[
                    (".->", EdgeStyle::DottedArrow),
                    (".-", EdgeStyle::DottedLine),
                ],
            ),
        ];
        for (opener, closers) in open_patterns {
            let Some(after) = s.strip_prefix(opener) else {
                continue;
            };
            let mut best: Option<(usize, &str, EdgeStyle)> = None;
            for (closer, style) in *closers {
                if let Some(idx) = after.find(closer) {
                    let better = match best {
                        Some((best_idx, best_closer, _)) => {
                            idx < best_idx || (idx == best_idx && closer.len() > best_closer.len())
                        }
                        None => true,
                    };
                    if better {
                        best = Some((idx, closer, *style));
                    }
                }
            }
            if let Some((idx, closer, style)) = best {
                let label = normalize_label(&after[..idx]);
                let total_len = opener.len() + idx + closer.len();
                return Ok((style, Some(label), total_len));
            }
        }

        Err(MermaidError::ParseError {
            line: self.current_line + 1,
            message: format!("Invalid edge syntax: {}", s),
        })
    }

    fn extract_node_id(&self, s: &str) -> String {
        let s = s.trim();
        let bytes = s.as_bytes();
        let mut in_quote = false;
        for (i, &b) in bytes.iter().enumerate() {
            if in_quote {
                if b == b'"' {
                    in_quote = false;
                }
                continue;
            }
            if b == b'"' {
                in_quote = true;
                continue;
            }
            if matches!(b, b'[' | b'(' | b'{' | b'<') {
                return s[..i].trim().to_string();
            }
        }
        s.to_string()
    }

    fn try_parse_node(&self, s: &str) -> Option<Node> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }

        if let Some(paren_paren_start) = s.find("((") {
            if s.ends_with("))") {
                let id = s[..paren_paren_start].trim().to_string();
                let label = normalize_label(&s[paren_paren_start + 2..s.len() - 2]);
                let id = if id.is_empty() {
                    label.chars().filter(|c| c.is_alphanumeric()).collect()
                } else {
                    id
                };
                return Some(Node {
                    id,
                    label: Some(label),
                    shape: NodeShape::Circle,
                });
            }
        }

        if let Some(bracket_paren_start) = s.find("([") {
            if s.ends_with("])") {
                let id = s[..bracket_paren_start].trim().to_string();
                let label = normalize_label(&s[bracket_paren_start + 2..s.len() - 2]);
                let id = if id.is_empty() {
                    label.chars().filter(|c| c.is_alphanumeric()).collect()
                } else {
                    id
                };
                return Some(Node {
                    id,
                    label: Some(label),
                    shape: NodeShape::Stadium,
                });
            }
        }

        if let Some(paren_bracket_start) = s.find("[(") {
            if s.ends_with(")]") {
                let id = s[..paren_bracket_start].trim().to_string();
                let label = normalize_label(&s[paren_bracket_start + 2..s.len() - 2]);
                let id = if id.is_empty() {
                    label.chars().filter(|c| c.is_alphanumeric()).collect()
                } else {
                    id
                };
                return Some(Node {
                    id,
                    label: Some(label),
                    shape: NodeShape::Cylinder,
                });
            }
        }

        if let Some(bracket_bracket_start) = s.find("[[") {
            if s.ends_with("]]") {
                let id = s[..bracket_bracket_start].trim().to_string();
                let label = normalize_label(&s[bracket_bracket_start + 2..s.len() - 2]);
                let id = if id.is_empty() {
                    label.chars().filter(|c| c.is_alphanumeric()).collect()
                } else {
                    id
                };
                return Some(Node {
                    id,
                    label: Some(label),
                    shape: NodeShape::Subroutine,
                });
            }
        }

        if let Some(brace_brace_start) = s.find("{{") {
            if s.ends_with("}}") {
                let id = s[..brace_brace_start].trim().to_string();
                let label = normalize_label(&s[brace_brace_start + 2..s.len() - 2]);
                let id = if id.is_empty() {
                    label.chars().filter(|c| c.is_alphanumeric()).collect()
                } else {
                    id
                };
                return Some(Node {
                    id,
                    label: Some(label),
                    shape: NodeShape::Hexagon,
                });
            }
        }

        if let Some(bracket_start) = s.find('[') {
            if s.ends_with(']') {
                let id = s[..bracket_start].trim().to_string();
                let label = normalize_label(&s[bracket_start + 1..s.len() - 1]);
                let id = if id.is_empty() {
                    label.chars().filter(|c| c.is_alphanumeric()).collect()
                } else {
                    id
                };
                return Some(Node {
                    id,
                    label: Some(label),
                    shape: NodeShape::Rectangle,
                });
            }
        }

        if let Some(paren_start) = s.find('(') {
            if s.ends_with(')') && !s.ends_with("))") {
                let id = s[..paren_start].trim().to_string();
                let label = normalize_label(&s[paren_start + 1..s.len() - 1]);
                let id = if id.is_empty() {
                    label.chars().filter(|c| c.is_alphanumeric()).collect()
                } else {
                    id
                };
                return Some(Node {
                    id,
                    label: Some(label),
                    shape: NodeShape::RoundedRectangle,
                });
            }
        }

        if let Some(brace_start) = s.find('{') {
            if s.ends_with('}') && !s.ends_with("}}") {
                let id = s[..brace_start].trim().to_string();
                let label = normalize_label(&s[brace_start + 1..s.len() - 1]);
                let id = if id.is_empty() {
                    label.chars().filter(|c| c.is_alphanumeric()).collect()
                } else {
                    id
                };
                return Some(Node {
                    id,
                    label: Some(label),
                    shape: NodeShape::Diamond,
                });
            }
        }

        if s.contains('>') && s.ends_with(']') {
            if let Some(gt_idx) = s.find('>') {
                let id = s[..gt_idx].trim().to_string();
                let label = normalize_label(&s[gt_idx + 1..s.len() - 1]);
                return Some(Node {
                    id,
                    label: Some(label),
                    shape: NodeShape::Asymmetric,
                });
            }
        }

        if s.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Some(Node {
                id: s.to_string(),
                label: None,
                shape: NodeShape::Rectangle,
            });
        }

        None
    }

    fn parse_subgraph(&mut self) -> Result<Subgraph, MermaidError> {
        let line = self
            .current_line_content()
            .ok_or_else(|| MermaidError::ParseError {
                line: self.current_line + 1,
                message: "Expected subgraph".to_string(),
            })?;

        let after_keyword = line.strip_prefix("subgraph ").unwrap_or("").trim();

        let (id, title) = if let Some(bracket_start) = after_keyword.find('[') {
            if after_keyword.ends_with(']') {
                let id = after_keyword[..bracket_start].trim().to_string();
                let title =
                    normalize_label(&after_keyword[bracket_start + 1..after_keyword.len() - 1]);
                (id, Some(title))
            } else {
                (after_keyword.to_string(), None)
            }
        } else if after_keyword.split_whitespace().count() > 1 {
            let id = format!("subGraph{}", self.next_subgraph_index);
            self.next_subgraph_index += 1;
            (id, Some(normalize_label(after_keyword)))
        } else {
            let id = after_keyword.to_string();
            (id, None)
        };

        self.advance();

        let statements = self.parse_statements()?;

        if self.current_line_content() == Some("end") {
            self.advance();
        }

        Ok(Subgraph {
            id,
            title,
            statements,
        })
    }

    fn parse_style(&mut self) -> Result<StyleStatement, MermaidError> {
        let line = self
            .current_line_content()
            .ok_or_else(|| MermaidError::ParseError {
                line: self.current_line + 1,
                message: "Expected style statement".to_string(),
            })?;

        let after_keyword = line.strip_prefix("style ").unwrap_or("").trim();
        let parts: Vec<&str> = after_keyword.splitn(2, ' ').collect();

        if parts.is_empty() {
            return Err(MermaidError::ParseError {
                line: self.current_line + 1,
                message: "Expected node id after 'style'".to_string(),
            });
        }

        let node_id = parts[0].to_string();
        let properties = if parts.len() > 1 {
            parse_style_properties(parts[1])
        } else {
            Vec::new()
        };

        self.advance();

        Ok(StyleStatement {
            node_id,
            properties,
        })
    }

    fn parse_class_def(&mut self) {
        let Some(line) = self.current_line_content() else {
            return;
        };
        let rest = line.strip_prefix("classDef ").unwrap_or(line).trim();
        if let Some((name, props_str)) = rest.split_once(char::is_whitespace) {
            let name = name.trim();
            if !name.is_empty() {
                self.class_defs
                    .insert(name.to_string(), parse_style_properties(props_str));
            }
        }
        self.advance();
    }

    fn apply_class_styles(&self, statements: &mut Vec<Statement>) {
        let mut styled = HashSet::new();
        collect_styled_node_ids(statements, &mut styled);
        for (node_id, classes) in &self.node_classes {
            if styled.contains(node_id) {
                continue;
            }
            let mut properties = Vec::new();
            for class in classes {
                if let Some(props) = self.class_defs.get(class) {
                    properties.extend(props.iter().cloned());
                }
            }
            if !properties.is_empty() {
                statements.push(Statement::Style(StyleStatement {
                    node_id: node_id.clone(),
                    properties,
                }));
            }
        }
    }
}

fn parse_style_properties(props_str: &str) -> Vec<(String, String)> {
    props_str
        .split(',')
        .filter_map(|prop| {
            let (k, v) = prop.split_once(':')?;
            let k = k.trim();
            let v = v.trim();
            if k.is_empty() || v.is_empty() {
                None
            } else {
                Some((k.to_string(), v.to_string()))
            }
        })
        .collect()
}

fn collect_styled_node_ids(statements: &[Statement], out: &mut HashSet<String>) {
    for stmt in statements {
        match stmt {
            Statement::Style(style) => {
                out.insert(style.node_id.clone());
            }
            Statement::Subgraph(subgraph) => {
                collect_styled_node_ids(&subgraph.statements, out);
            }
            _ => {}
        }
    }
}

fn is_ignored_flowchart_directive(line: &str) -> bool {
    let head = line.split_whitespace().next().unwrap_or("");
    matches!(
        head,
        "class" | "click" | "linkStyle" | "direction" | "accTitle" | "accDescr"
    )
}

fn split_ampersand_group(s: &str) -> Vec<&str> {
    let s = s.trim();
    if s.is_empty() {
        return Vec::new();
    }
    let bytes = s.as_bytes();
    let mut depth: usize = 0;
    let mut in_quote = false;
    let mut parts = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if in_quote {
            if b == b'"' {
                in_quote = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'"' => {
                in_quote = true;
                i += 1;
            }
            b'[' | b'(' | b'{' => {
                depth += 1;
                i += 1;
            }
            b']' | b')' | b'}' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b'&' if depth == 0
                && i > 0
                && bytes[i - 1].is_ascii_whitespace()
                && i + 1 < bytes.len()
                && bytes[i + 1].is_ascii_whitespace() =>
            {
                let part = s[start..i].trim();
                if !part.is_empty() {
                    parts.push(part);
                }
                start = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    let part = s[start..].trim();
    if !part.is_empty() {
        parts.push(part);
    }
    if parts.is_empty() {
        vec![s]
    } else {
        parts
    }
}

fn split_class_annotation(s: &str) -> (&str, Vec<String>) {
    let bytes = s.as_bytes();
    let mut depth: usize = 0;
    let mut in_quote = false;
    for i in 0..bytes.len() {
        let b = bytes[i];
        if in_quote {
            if b == b'"' {
                in_quote = false;
            }
            continue;
        }
        match b {
            b'"' => in_quote = true,
            b'[' | b'(' | b'{' => depth += 1,
            b']' | b')' | b'}' => depth = depth.saturating_sub(1),
            b':' if depth == 0
                && bytes.get(i + 1) == Some(&b':')
                && bytes.get(i + 2) == Some(&b':') =>
            {
                let body = s[..i].trim_end();
                let rest = s[i + 3..].trim();
                let classes: Vec<String> = rest
                    .split(',')
                    .map(str::trim)
                    .filter(|c| {
                        !c.is_empty()
                            && c.chars()
                                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
                    })
                    .map(str::to_string)
                    .collect();
                if classes.is_empty() {
                    return (s.trim(), Vec::new());
                }
                return (body, classes);
            }
            _ => {}
        }
    }
    (s.trim(), Vec::new())
}
