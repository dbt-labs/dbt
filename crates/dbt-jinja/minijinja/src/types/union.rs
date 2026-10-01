use std::collections::BTreeSet;

use crate::types::Type;

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UnionType {
    pub types: BTreeSet<Type>,
}

impl std::fmt::Debug for UnionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}]",
            self.types
                .iter()
                .map(|t| format!("{t:?}"))
                .collect::<Vec<_>>()
                .join("| ")
        )
    }
}

impl UnionType {
    pub fn new<I>(types: I) -> Self
    where
        I: IntoIterator<Item = Type>,
    {
        Self {
            types: types.into_iter().collect(),
        }
    }
}

impl UnionType {
    pub fn union(&self, other: &Type) -> Type {
        match other {
            Type::Any { hard: true } => Type::Any { hard: true },
            Type::Union(other_union) => {
                // Merge all types from both unions
                let mut all_types = BTreeSet::new();
                all_types.extend(self.types.iter().cloned());
                all_types.extend(other_union.types.iter().cloned());

                // Remove Any type if present
                all_types.remove(&Type::Any { hard: true });

                // Remove types that are subtypes of other types
                let filtered_types = Self::dedup_types(all_types);

                // Remove types that are subtypes of other types
                let filtered_types = Self::remove_subtypes(filtered_types);

                if filtered_types.is_empty() {
                    Type::Any { hard: true }
                } else if filtered_types.len() == 1 {
                    filtered_types.into_iter().next().unwrap()
                } else {
                    Type::Union(UnionType {
                        types: filtered_types,
                    })
                }
            }
            _ => {
                // Merge union with a single type
                let mut all_types = self.types.clone();
                all_types.insert(other.clone());

                // Remove types that are semanically equivalent to some other types in the set
                let filtered_types = Self::dedup_types(all_types);
                // Remove types that are subtypes of other types
                let filtered_types = Self::remove_subtypes(filtered_types);

                if filtered_types.is_empty() {
                    Type::Any { hard: true }
                } else if filtered_types.len() == 1 {
                    filtered_types.into_iter().next().unwrap()
                } else {
                    Type::Union(UnionType {
                        types: filtered_types,
                    })
                }
            }
        }
    }

    /// Remove types that are semanically equivalent to some other types in the set
    /// Keep the first occurrence of each type.
    fn dedup_types(types: BTreeSet<Type>) -> BTreeSet<Type> {
        let mut reps: Vec<Type> = Vec::new();
        let mut result = BTreeSet::new();

        for ty in types {
            let is_equivalent_to_existing = reps
                .iter()
                .any(|rep| rep == &ty || (rep.is_subtype_of(&ty) && ty.is_subtype_of(rep)));

            if !is_equivalent_to_existing {
                reps.push(ty.clone());
                result.insert(ty);
            }
        }

        result
    }

    /// Remove types that are subtypes of other types in the set
    fn remove_subtypes(types: BTreeSet<Type>) -> BTreeSet<Type> {
        let mut result = BTreeSet::new();

        for candidate in &types {
            let mut is_subtype_of_another = false;

            for other in &types {
                let sub1 = candidate.is_subtype_of(other);
                let sub2 = other.is_subtype_of(candidate);
                // sub1 && sub2 implies equality we have to account for in case
                // semantic equality is not the same as pointer equality.
                if candidate != other && sub1 && !sub2 {
                    is_subtype_of_another = true;
                    break;
                }
            }

            if !is_subtype_of_another {
                result.insert(candidate.clone());
            }
        }

        result
    }

    pub fn is_optional(&self) -> bool {
        self.types.iter().any(|t| matches!(t, Type::None))
    }

    pub fn exclude(&self, other: &Type) -> Type {
        let mut types = self.types.clone();
        types.remove(other);
        if types.is_empty() {
            panic!("UnionType::exclude: types is empty");
        } else if types.len() == 1 {
            types.into_iter().next().unwrap()
        } else {
            Type::Union(UnionType { types })
        }
    }

    pub fn get_non_optional_type(&self) -> Type {
        self.exclude(&Type::None)
    }
}
