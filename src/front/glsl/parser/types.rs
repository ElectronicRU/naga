use crate::{
    front::glsl::{
        ast::{StorageQualifier, StructLayout, TypeQualifier},
        error::ExpectedToken,
        parser::ParsingContext,
        token::{SourceMetadata, TokenValue},
        Error, ErrorKind, Parser, Result,
    },
    ArraySize, Handle, StorageClass, Type, TypeInner,
};

impl<'source> ParsingContext<'source> {
    /// Parses an optional array_specifier returning `Ok(None)` if there is no
    /// LeftBracket
    pub fn parse_array_specifier(&mut self, parser: &mut Parser) -> Result<Option<ArraySize>> {
        if self.bump_if(parser, TokenValue::LeftBracket).is_some() {
            if self.bump_if(parser, TokenValue::RightBracket).is_some() {
                return Ok(Some(ArraySize::Dynamic));
            }

            let (constant, _) = self.parse_constant_expression(parser)?;
            self.expect(parser, TokenValue::RightBracket)?;
            Ok(Some(ArraySize::Constant(constant)))
        } else {
            Ok(None)
        }
    }

    pub fn parse_type(
        &mut self,
        parser: &mut Parser,
    ) -> Result<(Option<Handle<Type>>, SourceMetadata)> {
        let token = self.bump(parser)?;
        let handle = match token.value {
            TokenValue::Void => None,
            TokenValue::TypeName(ty) => Some(parser.module.types.fetch_or_append(ty)),
            TokenValue::Struct => {
                let ty_name = self.expect_ident(parser)?.0;
                self.expect(parser, TokenValue::LeftBrace)?;
                let mut members = Vec::new();
                let span =
                    self.parse_struct_declaration_list(parser, &mut members, StructLayout::Std140)?;
                self.expect(parser, TokenValue::RightBrace)?;

                let ty = parser.module.types.append(Type {
                    name: Some(ty_name.clone()),
                    inner: TypeInner::Struct {
                        top_level: false,
                        members,
                        span,
                    },
                });
                parser.lookup_type.insert(ty_name, ty);
                Some(ty)
            }
            TokenValue::Identifier(ident) => match parser.lookup_type.get(&ident) {
                Some(ty) => Some(*ty),
                None => {
                    return Err(Error {
                        kind: ErrorKind::UnknownType(ident),
                        meta: token.meta,
                    })
                }
            },
            _ => {
                return Err(Error {
                    kind: ErrorKind::InvalidToken(
                        token.value,
                        vec![
                            TokenValue::Void.into(),
                            TokenValue::Struct.into(),
                            ExpectedToken::TypeName,
                        ],
                    ),
                    meta: token.meta,
                });
            }
        };

        let size = self.parse_array_specifier(parser)?;
        Ok((handle.map(|ty| parser.maybe_array(ty, size)), token.meta))
    }

    pub fn parse_type_non_void(
        &mut self,
        parser: &mut Parser,
    ) -> Result<(Handle<Type>, SourceMetadata)> {
        let (maybe_ty, meta) = self.parse_type(parser)?;
        let ty = maybe_ty.ok_or_else(|| Error {
            kind: ErrorKind::SemanticError("Type can't be void".into()),
            meta,
        })?;

        Ok((ty, meta))
    }

    pub fn peek_type_qualifier(&mut self, parser: &mut Parser) -> bool {
        self.peek(parser).map_or(false, |t| match t.value {
            TokenValue::Interpolation(_)
            | TokenValue::Sampling(_)
            | TokenValue::PrecisionQualifier(_)
            | TokenValue::Const
            | TokenValue::In
            | TokenValue::Out
            | TokenValue::Uniform
            | TokenValue::Buffer
            | TokenValue::Restrict
            | TokenValue::StorageAccess(_)
            | TokenValue::Layout => true,
            _ => false,
        })
    }

    pub fn parse_type_qualifiers(
        &mut self,
        parser: &mut Parser,
    ) -> Result<Vec<(TypeQualifier, SourceMetadata)>> {
        let mut qualifiers = Vec::new();

        while self.peek_type_qualifier(parser) {
            let token = self.bump(parser)?;

            // Handle layout qualifiers outside the match since this can push multiple values
            if token.value == TokenValue::Layout {
                self.parse_layout_qualifier_id_list(parser, &mut qualifiers)?;
                continue;
            }

            qualifiers.push((
                match token.value {
                    TokenValue::Interpolation(i) => TypeQualifier::Interpolation(i),
                    TokenValue::Const => TypeQualifier::StorageQualifier(StorageQualifier::Const),
                    TokenValue::In => TypeQualifier::StorageQualifier(StorageQualifier::Input),
                    TokenValue::Out => TypeQualifier::StorageQualifier(StorageQualifier::Output),
                    TokenValue::Uniform => TypeQualifier::StorageQualifier(
                        StorageQualifier::StorageClass(StorageClass::Uniform),
                    ),
                    TokenValue::Buffer => TypeQualifier::StorageQualifier(
                        StorageQualifier::StorageClass(StorageClass::Storage {
                            access: crate::StorageAccess::default(),
                        }),
                    ),
                    TokenValue::Sampling(s) => TypeQualifier::Sampling(s),
                    TokenValue::PrecisionQualifier(p) => TypeQualifier::Precision(p),
                    TokenValue::StorageAccess(access) => TypeQualifier::StorageAccess(access),
                    TokenValue::Restrict => continue,
                    _ => unreachable!(),
                },
                token.meta,
            ))
        }

        Ok(qualifiers)
    }

    pub fn parse_layout_qualifier_id_list(
        &mut self,
        parser: &mut Parser,
        qualifiers: &mut Vec<(TypeQualifier, SourceMetadata)>,
    ) -> Result<()> {
        self.expect(parser, TokenValue::LeftParen)?;
        loop {
            self.parse_layout_qualifier_id(parser, qualifiers)?;

            if self.bump_if(parser, TokenValue::Comma).is_some() {
                continue;
            }

            break;
        }
        self.expect(parser, TokenValue::RightParen)?;

        Ok(())
    }

    pub fn parse_layout_qualifier_id(
        &mut self,
        parser: &mut Parser,
        qualifiers: &mut Vec<(TypeQualifier, SourceMetadata)>,
    ) -> Result<()> {
        // layout_qualifier_id:
        //     IDENTIFIER
        //     IDENTIFIER EQUAL constant_expression
        //     SHARED
        let mut token = self.bump(parser)?;
        match token.value {
            TokenValue::Identifier(name) => {
                if self.bump_if(parser, TokenValue::Assign).is_some() {
                    let (value, end_meta) = self.parse_uint_constant(parser)?;
                    token.meta = token.meta.union(&end_meta);

                    qualifiers.push((
                        match name.as_str() {
                            "location" => TypeQualifier::Location(value),
                            "set" => TypeQualifier::Set(value),
                            "binding" => TypeQualifier::Binding(value),
                            "local_size_x" => TypeQualifier::WorkGroupSize(0, value),
                            "local_size_y" => TypeQualifier::WorkGroupSize(1, value),
                            "local_size_z" => TypeQualifier::WorkGroupSize(2, value),
                            _ => {
                                parser.errors.push(Error {
                                    kind: ErrorKind::UnknownLayoutQualifier(name),
                                    meta: token.meta,
                                });
                                return Ok(());
                            }
                        },
                        token.meta,
                    ))
                } else {
                    qualifiers.push((
                        match name.as_str() {
                            "push_constant" => {
                                qualifiers.push((
                                    TypeQualifier::Layout(StructLayout::Std430),
                                    token.meta,
                                ));
                                qualifiers.push((
                                    TypeQualifier::StorageQualifier(
                                        StorageQualifier::StorageClass(StorageClass::PushConstant),
                                    ),
                                    token.meta,
                                ));
                                return Ok(());
                            }
                            "std140" => TypeQualifier::Layout(StructLayout::Std140),
                            "std430" => TypeQualifier::Layout(StructLayout::Std430),
                            "early_fragment_tests" => TypeQualifier::EarlyFragmentTests,
                            _ => {
                                parser.errors.push(Error {
                                    kind: ErrorKind::UnknownLayoutQualifier(name),
                                    meta: token.meta,
                                });
                                return Ok(());
                            }
                        },
                        token.meta,
                    ));
                };
            }
            // TODO: handle Shared?
            _ => parser.errors.push(Error {
                kind: ErrorKind::InvalidToken(token.value, vec![ExpectedToken::Identifier]),
                meta: token.meta,
            }),
        }

        Ok(())
    }

    pub fn peek_type_name(&mut self, parser: &mut Parser) -> bool {
        self.peek(parser).map_or(false, |t| match t.value {
            TokenValue::TypeName(_) | TokenValue::Void => true,
            TokenValue::Struct => true,
            TokenValue::Identifier(ref ident) => parser.lookup_type.contains_key(ident),
            _ => false,
        })
    }
}
