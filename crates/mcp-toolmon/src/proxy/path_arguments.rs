//! Registry of MCP tool path arguments validated without altering calls.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathArgumentKind {
    Workspace,
    Path,
    RenameDestination,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PathArgument {
    pub(crate) name: &'static str,
    pub(crate) kind: PathArgumentKind,
}

const PATH_ARGUMENT_REGISTRY: &[(&str, &str, PathArgumentKind)] = &[
    ("fs_list_dir", "path", PathArgumentKind::Path),
    ("fs_stat", "path", PathArgumentKind::Path),
    ("fs_move_file", "from", PathArgumentKind::Path),
    ("fs_move_file", "to", PathArgumentKind::Path),
    ("fs_move_file", "root", PathArgumentKind::Path),
    ("fs_rename_file", "from", PathArgumentKind::Path),
    ("fs_rename_file", "to", PathArgumentKind::RenameDestination),
    ("fs_rename_file", "root", PathArgumentKind::Path),
    ("fs_copy_file", "from", PathArgumentKind::Path),
    ("fs_copy_file", "to", PathArgumentKind::Path),
    ("fs_copy_file", "root", PathArgumentKind::Path),
    ("fs_delete_file", "path", PathArgumentKind::Path),
    ("fs_delete_file", "root", PathArgumentKind::Path),
    ("fs_delete_dir", "path", PathArgumentKind::Path),
    ("fs_delete_dir", "root", PathArgumentKind::Path),
    ("peek_read", "path", PathArgumentKind::Path),
    ("peek_grep", "path", PathArgumentKind::Path),
    ("peek_count", "path", PathArgumentKind::Path),
    ("peek_skeleton", "path", PathArgumentKind::Path),
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
        .find(|(registered_tool, argument, _)| *registered_tool == tool && *argument == name)
        .map(|(_, name, kind)| PathArgument { name, kind: *kind })
}