use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[allow(
                clippy::new_without_default,
                reason = "default IDs must not be implicit"
            )]
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            #[must_use]
            pub const fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            #[must_use]
            pub const fn nil() -> Self {
                Self(Uuid::nil())
            }

            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value).map(Self)
            }
        }
    };
}

uuid_id!(UserId);
uuid_id!(RunnerId);
uuid_id!(WorkspaceId);
uuid_id!(SessionId);
uuid_id!(ApprovalId);
uuid_id!(QuestionId);
uuid_id!(ConnectorBindingId);
uuid_id!(TurnId);
uuid_id!(ItemId);
uuid_id!(PlanId);
uuid_id!(DiffId);
uuid_id!(ArtifactId);
uuid_id!(CommandId);

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use crate::{ArtifactId, CommandId, DiffId, ItemId, PlanId, TurnId};

    #[test]
    fn universal_uuid_ids_round_trip_as_strings() {
        let raw = "00000000-0000-0000-0000-000000000042";

        let ids = [
            serde_json::to_value(TurnId::from_str(raw).expect("parse turn id"))
                .expect("serialize turn id"),
            serde_json::to_value(ItemId::from_str(raw).expect("parse item id"))
                .expect("serialize item id"),
            serde_json::to_value(PlanId::from_str(raw).expect("parse plan id"))
                .expect("serialize plan id"),
            serde_json::to_value(DiffId::from_str(raw).expect("parse diff id"))
                .expect("serialize diff id"),
            serde_json::to_value(ArtifactId::from_str(raw).expect("parse artifact id"))
                .expect("serialize artifact id"),
            serde_json::to_value(CommandId::from_str(raw).expect("parse command id"))
                .expect("serialize command id"),
        ];

        assert!(ids.iter().all(|json| json == raw));
    }
}
