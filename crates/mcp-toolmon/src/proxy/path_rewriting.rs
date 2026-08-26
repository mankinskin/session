//! Registry of MCP tool arguments that name filesystem paths, used to rewrite
//! `workspace`/`path` arguments to a resolved session checkout before
//! forwarding a call.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathArgumentKind {
    Workspace,
    Path,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PathArgument {
    pub(crate) name: &'static str,
    pub(crate) kind: PathArgumentKind,
}

const PATH_ARGUMENT_REGISTRY: &[(&str, PathArgument)] = &[
    (
        "fs_list_dir",
        PathArgument {
            name: "path",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_stat",
        PathArgument {
            name: "path",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_move_file",
        PathArgument {
            name: "from",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_move_file",
        PathArgument {
            name: "to",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_move_file",
        PathArgument {
            name: "root",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_rename_file",
        PathArgument {
            name: "from",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_rename_file",
        PathArgument {
            name: "root",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_copy_file",
        PathArgument {
            name: "from",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_copy_file",
        PathArgument {
            name: "to",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_copy_file",
        PathArgument {
            name: "root",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_delete_file",
        PathArgument {
            name: "path",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_delete_file",
        PathArgument {
            name: "root",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_delete_dir",
        PathArgument {
            name: "path",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "fs_delete_dir",
        PathArgument {
            name: "root",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "peek_read",
        PathArgument {
            name: "path",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "peek_grep",
        PathArgument {
            name: "path",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "peek_count",
        PathArgument {
            name: "path",
            kind: PathArgumentKind::Path,
        },
    ),
    (
        "peek_skeleton",
        PathArgument {
            name: "path",
            kind: PathArgumentKind::Path,
        },
    ),
];

pub(crate) fn registered_path_argument(
    tool: &str,
    name: &str,
) -> Option<PathArgument> {
    if name == "workspace" {
        return Some(PathArgument {
            name: "workspace",
            kind: PathArgumentKind::Workspace,
        });
    }
    PATH_ARGUMENT_REGISTRY
        .iter()
        .find(|(registered_tool, argument)| {
            *registered_tool == tool && argument.name == name
        })
        .map(|(_, argument)| *argument)
}
