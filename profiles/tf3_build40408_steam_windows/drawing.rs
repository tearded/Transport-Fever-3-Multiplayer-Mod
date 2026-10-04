//! Native data for Steam Windows build 40408. See hooks.toml for identity.

/// The profile's names of what this module calls, redirects or detours.
pub const CREATE_TARGET: &str = "UI::RendererFactory::Create";

pub const ADD_TARGET: &str = "UI::CRendererComponent::AddRenderable";

pub const REMOVE_TARGET: &str = "UI::CRendererComponent::RemoveRenderable";

pub const CLEAR_TARGET: &str = "UI::BuilderRenderer::Clear";

pub const DESTROY_TARGET: &str = "UI::BuilderRenderer::vf0";

pub const FILL_TARGET: &str = "builder_renderer_util::AddToRenderer";

pub const EVALUATE_TARGET: &str = "CreateProposalData";

pub const CALL_TARGET: &str = "makeProposalData/CreateProposalData call";

pub const GAME_UI_DTOR_TARGET: &str = "UI::CGameUI::~CGameUI";

pub const END_HEIGHTS_TARGET: &str = "UI::BuilderRenderer::EndHeightMod";

pub const APPLY_TARGET: &str = "terrain::ViewTerrain::ApplyBlocks";

/// The layout anchors, and the opcode each one's offset follows.
pub const GAME_UI_FIELD: (&str, &[u8]) =
    ("UI::CMenuUI::StartGame/CGameUI store", &[0x48, 0x89, 0x83]);

pub const FACTORY_FIELD: (&str, &[u8]) = (
    "UI::CGameUI::CreateUI/RendererFactory field",
    &[0x49, 0x8D, 0x84, 0x24],
);

pub const MAIN_VIEW_FIELD: (&str, &[u8]) = (
    "UI::CGameUI::CreateUI/mainView store",
    &[0x49, 0x89, 0x8C, 0x24],
);

pub const MODEL_DATA_FIELD: (&str, &[u8]) = ("ProposalViewer/ModelData read", &[0x48, 0x8B, 0x89]);


pub const UPLOAD_FIELD: (&str, &[u8]) =
    ("BuilderRenderer::EndHeightMod/upload flag", &[0x80, 0xB9]);

/// Bytes of the upload anchor [`crate::drawing::Upload::read`] reads.
pub const UPLOAD_LEN: usize = 40;
