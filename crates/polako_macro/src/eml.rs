use std::collections::HashMap;
use constructivist::{context::Context, proc::*, throw};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::{
    braced, bracketed,
    parse::Parse,
    spanned::Spanned,
    token::{self, Bracket},
    Lit, LitStr, Token, parenthesized, parse_quote,
};
use crate::{variant::Variant, exts::*};

pub trait ParamsExt {
    fn build_patch(
        &self,
        _: &EmlContext,
        tag: &Ident,
        this: &TokenStream,
        patch_empty: bool,
    ) -> syn::Result<TokenStream>;
    fn build_construct(
        &self,
        ctx: &EmlContext,
        tag: &Ident,
        flattern: bool,
    ) -> syn::Result<TokenStream>;
}

impl ParamsExt for Params<Variant> {
    fn build_patch(
        &self,
        ctx: &EmlContext,
        tag: &Ident,
        this: &TokenStream,
        patch_empty: bool,
    ) -> syn::Result<TokenStream> {
        let mut body = quote! {};
        if !patch_empty && self.items.is_empty() {
            return Ok(body);
        }
        for arg in self.items.iter() {
            let ident = &arg.ident;
            let value = Variant::build(&arg.value, ctx)?;
            // let value = Variant::build(, ctx)?;
            body = quote! { #body
                __component__.#ident = #value.into();
            }
        }
        Ok(quote! {{
            let mut __entity__ = world.entity_mut(#this);
            if !__entity__.contains::<#tag>() {
                __entity__.insert(#tag::default());
            }
            let mut __component__ = __entity__.get_mut::<#tag>().unwrap();
            #body
        }})
    }
    fn build_construct(
        &self,
        ctx: &EmlContext,
        tag: &Ident,
        flattern: bool,
    ) -> syn::Result<TokenStream> {
        let construct = Construct {
            flattern,
            ty: syn::parse2(quote! { #tag })?,
            params: self.clone(),
        };
        construct.build(&ctx.context)
    }
}

pub enum EmlPathPart {
    /// `hidden` in `.class[hidden]`
    Index(Ident),
    /// `color` in `.bind.color`
    Prop(Ident),
}
impl Parse for EmlPathPart {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        if input.peek(Bracket) {
            let content;
            bracketed!(content in input);
            Ok(EmlPathPart::Index(content.parse()?))
        } else {
            Ok(EmlPathPart::Prop(input.parse()?))
        }
    }
}
pub struct EmlPath(Vec<EmlPathPart>);
impl Parse for EmlPath {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut parts = vec![];
        let mut dot = if input.peek(Token![.]) {
            Some(input.parse::<Token![.]>()?)
        } else {
            None
        };
        while let Ok(part) = input.parse() {
            parts.push(part);
            dot = if input.peek(Token![.]) {
                Some(input.parse::<Token![.]>()?)
            } else {
                None
            };
        }
        if dot.is_some() {
            parts.push(EmlPathPart::Prop(format_ident!(
                "DOT_AUTOCOMPLETE_TOKEN",
                span = dot.span()
            )));
        }
        if parts.is_empty() {
            throw!(input, "EmlPath should contain at least one part");
        }
        Ok(EmlPath(parts))
    }
}

pub struct EmlParam {
    pub extension: Ident,
    pub path: EmlPath,
    pub value: Option<Variant>,
}

impl Parse for EmlParam {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        input.parse::<Token![.]>()?;
        let extension = input.parse()?;
        let path = input.parse()?;
        let value = if input.peek(Token![:]) {
            input.parse::<Token![:]>()?;
            Some(input.parse()?)
        } else {
            None
        };
        Ok(EmlParam {
            extension,
            path,
            value,
        })
    }
}

impl EmlParam {
    pub fn build_extension(
        &self,
        ctx: &EmlContext,
        tag: &Ident,
        entity: &TokenStream,
    ) -> syn::Result<TokenStream> {
        let cst = ctx.constructivism();
        let ext_ident = &self.extension;
        let mut ext = quote! {
            <<#tag as #cst::Construct>::Design as #cst::Singleton>::instance().#ext_ident()
        };
        for part in self.path.0.iter() {
            ext = match part {
                EmlPathPart::Prop(ident) => {
                    quote! { #ext.#ident() }
                }
                EmlPathPart::Index(ident) => {
                    let ident = ident.to_string();
                    quote! { #ext.at(#ident) }
                }
            }
        }
        // Ok(quote! { #ext; })
        if let Some(value) = &self.value {
            let value = Variant::build(value, ctx)?;
            let assign = quote_spanned! { value.span()=>
                __ext__.assign(#entity, #value)
            };
            Ok(quote! {{
                let __ext__ = #ext;
                #assign;
            }})
        } else {
            Ok(quote! { #ext.declare(#entity); })
        }
    }
}

// pub struct EmplParams(Vec<EmlParam>)

pub struct EmlParams {
    common: Params<Variant>,
    extended: Vec<EmlParam>,
}

impl Parse for EmlParams {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut common = vec![];
        let mut extended = vec![];
        while !input.is_empty() {
            if input.fork().parse::<EmlParam>().is_ok() {
                extended.push(input.parse()?);
            } else {
                common.push(input.parse()?);
            }
            // if input.fork().parse::<Param<Variant>>().is_ok() {
            //     common.push(input.parse()?);
            // } else {
            //     extended.push(input.parse()?);
            // }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(EmlParams {
            extended,
            common: Params { items: common },
        })
    }
}

impl EmlParams {
    pub fn build_construct(&self, ctx: &EmlContext, tag: &Ident) -> syn::Result<TokenStream> {
        self.common.build_construct(ctx, tag, false)
    }

    pub fn build_extensions(
        &self,
        ctx: &EmlContext,
        tag: &Ident,
        entity: &TokenStream,
    ) -> syn::Result<TokenStream> {
        let mut out = quote! {};
        for param in self.extended.iter() {
            let ext = param.build_extension(ctx, tag, entity)?;
            out = quote! { #out #ext };
        }
        Ok(out)
    }

    pub fn braced(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let content;
        braced!(content in input);
        content.parse()
    }
    pub fn empty() -> Self {
        EmlParams {
            common: Params::empty(),
            extended: vec![],
        }
    }
}

pub struct BindPath {
    path: Vec<Ident>,
    map: Option<BindMap>,
}

impl Parse for BindPath {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut path = vec![];
        let mut map = None;
        loop {
            if let Some(prop_map) = input.parse_prop_map(&mut path)? {
                map = Some(prop_map);
                break;
            }
            path.push(input.parse()?);
            let dot = if input.peek(Token![.]) {
                Some(input.parse::<Token![.]>()?)
            } else {
                None
            };
            if input.is_empty() || input.peek_bind_direction() {
                if let Some(dot) = dot {
                    path.push(format_ident!(
                        "DOT_AUTOCOMPLETE_TOKEN",
                        span = dot.span()
                    ));
                }
                break;
            }
        }
        Ok(BindPath { path, map })

    }
}

pub enum BindDirection {
    Forward,
    Backward,
    Both,
}

impl Parse for BindDirection {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        if input.peek(Token![=>]) {
            input.parse::<Token![=>]>()?;
            Ok(BindDirection::Forward)
        } else if input.peek(Token![<=]) {
            input.parse::<Token![<=]>()?;
            Ok(BindDirection::Backward)
        } else if input.peek(Token![<=]) && input.peek2(Token![=>]) {
            input.parse::<Token![<=]>()?;
            input.parse::<Token![=>]>()?;
            Ok(BindDirection::Both)
        } else {
            throw!(input, "Expected bind direction '=>', '<=' or '<==>'");
        }
    }
}

pub struct Bind {
    from: BindPath,
    to: BindPath,
    #[allow(unused)]
    bidirectional: bool
}

impl Parse for Bind {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let first: BindPath = input.parse()?;
        if let Ok(direction) = input.parse::<BindDirection>() {
            let second: BindPath = input.parse()?;
            Ok(match direction {
                BindDirection::Forward => Bind {
                    from: first,
                    to: second,
                    bidirectional: false,
                },
                BindDirection::Backward => Bind {
                    from: second,
                    to: first,
                    bidirectional: true,
                },
                BindDirection::Both => Bind {
                    from: first,
                    to: second,
                    bidirectional: true,
                }
            })
        } else {
            let second = BindPath {
                map: None,
                path: vec![
                    format_ident!("DOT_AUTOCOMPLETE_TOKEN"),
                    format_ident!("DOT_AUTOCOMPLETE_TOKEN"),
                ]
            };
            Ok(Bind { from: first, to: second, bidirectional: false })
        }
    }
}

pub enum EmlDirective {
    Resource(Ident, Ident),
    Bind(Bind),
    None,
}

impl EmlDirective {
    pub fn is_none(&self) -> bool {
        match  self {
            EmlDirective::None => true,
            _ => false,
        }
    }
}

impl Parse for EmlDirective {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        if !(input.peek(syn::Ident) && input.peek2(token::Paren)) {
            return Ok(EmlDirective::None);
        }
        let ident = input.parse::<Ident>()?;
        Ok(match ident.to_string().as_str() {
            "resource" => {
                let content;
                parenthesized!(content in input);
                let ident = content.parse()?;
                content.parse::<Token![,]>()?;
                let ty = content.parse()?;
                if input.peek(Token![;]) {
                    input.parse::<Token![;]>()?;
                }
                EmlDirective::Resource(ident, ty)
            },
            "bind" => {
                let content;
                parenthesized!(content in input);
                let bind = content.parse()?;
                if input.peek(Token![;]) {
                    input.parse::<Token![;]>()?;
                }
                EmlDirective::Bind(bind)

            },
            _ => {
                throw!(ident, "Unknown dirrective");
            }
        })
    }
}

pub enum EmlChild {
    Literal(LitStr),
    Node(EmlNode),
}

impl Parse for EmlChild {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        if let Ok(lit) = input.parse::<Lit>() {
            if let Lit::Str(val) = lit {
                Ok(EmlChild::Literal(val.clone()))
            } else {
                throw!(lit, "Only string literals supported");
            }
        } else {
            Ok(EmlChild::Node(input.parse()?))
        }
    }
}

pub enum EmlContent {
    Provided(Ident),
    Declared(Vec<EmlChild>),
}

impl EmlContent {
    pub fn build(&self, ctx: &EmlContext, tag: &Ident) -> syn::Result<TokenStream> {
        let cst = &ctx.path("constructivism");
        Ok(match self {
            EmlContent::Provided(ident) => quote! { #ident },
            EmlContent::Declared(children) => {
                let size = children.len();
                let mut content = quote! {};
                for child in children.iter() {
                    content = match child {
                        EmlChild::Literal(lit) => {
                            let assign = quote_spanned! { lit.span()=>
                                let _: Implemented =
                                    <<#tag as #cst::Construct>::Design as #cst::Singleton>::instance()
                                        .push_text(world, &mut __content__, #lit);
                            };
                            quote! { #content #assign }
                        }
                        EmlChild::Node(ch) => {
                            let span = ch.tag.span();
                            let ct = ch.build(ctx, false)?;
                            let assign = quote_spanned! { span=>
                                let _: Implemented =
                                    <<#tag as #cst::Construct>::Design as #cst::Singleton>::instance()
                                        .push_content(world, &mut __content__, __content_item__);
                            };
                            quote! { #content
                                let __content_item__ = { #ct };
                                #assign
                            }
                        }
                    }
                }
                quote! {
                    {
                        let mut __content__ = ::std::vec::Vec::<_>::new();
                        __content__.reserve(#size);
                        #content
                        __content__
                    }
                }
            }
        })
    }
}

impl Parse for EmlContent {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        Ok(if input.peek(token::Bracket) {
            let content;
            bracketed!(content in input);
            if content.peek(token::Bracket) {
                let content2;
                bracketed!(content2 in content);
                EmlContent::Provided(content2.parse()?)
            } else {
                let mut children = vec![];
                for child in content.parse_terminated(EmlChild::parse, Token![,])? {
                    children.push(child);
                }
                EmlContent::Declared(children)
            }
        } else {
            EmlContent::Declared(vec![])
        })
    }
}

// Patch component on current entity, everythinng after + in
// Div + Style(width: Val::Percent(100.))
pub struct EmlPatch {
    pub ident: Ident,
    pub items: Params<Variant>,
}

impl Parse for EmlPatch {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let ident: Ident = input.parse()?;
        let items = Params::parenthesized(input)?;
        Ok(EmlPatch { ident, items })
    }
}

impl EmlPatch {
    pub fn build(&self, ctx: &EmlContext, this: &TokenStream) -> syn::Result<TokenStream> {
        self.items.build_patch(ctx, &self.ident, this, true)
    }
}

// Add new component to the current entity, everything after ++ in
// Div + Style
pub struct EmlComponent {
    pub ident: Ident,
    pub items: Params<Variant>,
}

impl Parse for EmlComponent {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let ident = input.parse()?;
        let items = if input.peek(token::Brace) {
            Params::braced(input)?
        } else {
            Params::empty()
        };
        Ok(EmlComponent { ident, items })
    }
}

impl EmlComponent {
    pub fn build(&self, ctx: &EmlContext, this: &TokenStream) -> syn::Result<TokenStream> {
        let construct = self.items.build_construct(ctx, &self.ident, false)?;
        let cst = ctx.path("constructivism");
        Ok(quote! {
            world.entity_mut(#this).insert(#cst::Flattern::flattern(#construct));
        })
    }
}

pub enum EmlMixin {
    Patch(EmlPatch),
    Component(EmlComponent),
}

impl Parse for EmlMixin {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        if input.fork().parse::<EmlPatch>().is_ok() {
            Ok(EmlMixin::Patch(input.parse()?))
        } else if input.fork().parse::<EmlComponent>().is_ok() {
            Ok(EmlMixin::Component(input.parse()?))
        } else {
            throw!(input.span(), "Unexpected input");
        }
    }
}

pub struct EmlMixins(pub Vec<EmlMixin>);
impl Parse for EmlMixins {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut mixins = vec![];
        while input.peek(Token![+]) {
            input.parse::<Token![+]>()?;
            mixins.push(input.parse()?);
        }
        Ok(EmlMixins(mixins))
    }
}

impl EmlMixins {
    pub fn build(&self, ctx: &EmlContext, this: &TokenStream) -> syn::Result<TokenStream> {
        let mut out = quote! {};
        for mixin in self.0.iter() {
            out = match mixin {
                EmlMixin::Patch(patch) => {
                    let patch = patch.build(ctx, this)?;
                    quote! { #out #patch }
                }
                EmlMixin::Component(component) => {
                    let construct = component.build(ctx, this)?;
                    quote! { #out #construct }
                }
            };
        }
        Ok(out)
    }
}

pub enum EmlRoot {
    Element(EmlNode),
    Base {
        tag: Ident,
        overrides: Params<Variant>,
        mixins: EmlMixins,
        children: EmlContent,
    },
}

impl Parse for EmlRoot {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        if input.peek(syn::Ident) && input.peek2(Token![:]) && input.peek3(Token![:]) {
            let tag = input.parse::<Ident>()?;
            input.parse::<Token![:]>()?;
            input.parse::<Token![:]>()?;
            let sup = input.parse::<Ident>()?;
            if &sup.to_string() != "Base" {
                throw!(sup, "Expected Base");
            }
            let overrides = if input.peek(token::Paren) {
                Params::parenthesized(input)?
            } else {
                Params::empty()
            };
            let mixins = input.parse()?;
            let children = input.parse()?;
            Ok(EmlRoot::Base {
                tag,
                overrides,
                mixins,
                children,
            })
        } else {
            // throw!(node_input, format!("parsing node {}",node_input.to_string()) );
            Ok(EmlRoot::Element(input.parse()?))
        }
    }
}

pub struct EmlContext {
    context: Context,
    variables: HashMap<Ident, Variable>,
    strict: bool,
}

impl std::ops::Deref for EmlContext {
    type Target = Context;
    fn deref(&self) -> &Self::Target {
        &self.context
    }
}

impl EmlRoot {
    pub fn tag(&self) -> Ident {
        match self {
            EmlRoot::Element(elem) => elem.tag.clone(),
            EmlRoot::Base { tag, .. } => tag.clone(),
        }
    }
    pub fn fetch_variables(&self, variables: &mut HashMap<Ident, Variable>) -> syn::Result<()> {
        match self {
            EmlRoot::Element(node) => node.fetch_variables(variables, true),
            EmlRoot::Base {
                children: EmlContent::Declared(items),
                ..
            } => {
                for item in items.iter() {
                    if let EmlChild::Node(node) = item {
                        node.fetch_variables(variables, false)?
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    pub fn build(&self, ctx: &EmlContext) -> syn::Result<TokenStream> {
        match self {
            EmlRoot::Base {
                tag,
                overrides,
                mixins,
                children,
            } => {
                if !ctx.strict {
                    throw!(
                        tag,
                        "Tag::Base only available as root inside the build! macro."
                    );
                }
                self.build_super(ctx, tag, overrides, mixins, children)
            }
            EmlRoot::Element(node) => {
                if ctx.strict {
                    throw!(
                        node.tag,
                        "Only Tag::Base available as root inside the build! macro."
                    );
                }
                let eml = &ctx.path("eml");
                let body = node.build(ctx, true)?;
                let tag = &node.tag;
                Ok(quote! {
                    let __root_model__ = #eml::Model::<#tag>::new(__root__);
                    #body
                })
            }
        }
    }

    fn build_super(
        &self,
        ctx: &EmlContext,
        tag: &Ident,
        overrides: &Params<Variant>,
        mixins: &EmlMixins,
        content: &EmlContent,
    ) -> syn::Result<TokenStream> {
        let eml = &ctx.path("eml");
        let cst = &ctx.path("constructivism");
        let build_content = content.build(ctx, tag)?;
        let apply_patches = overrides.build_patch(ctx, tag, &quote! { __root__ }, false)?;
        let apply_mixins = mixins.build(ctx, &quote! { __root__ })?;

        Ok(quote! {
            let __root_model__ = #eml::Model::<#tag>::new(__root__);
            #apply_patches;
            <<#tag as #cst::Construct>::Base as #eml::Element>::build_element(#build_content)
                .eml()
                .write(world, __root__);
            #apply_mixins
        })
    }
}

pub struct EmlNode {
    pub tag: Ident,
    pub model: Option<Ident>,
    pub args: EmlParams,
    pub mixins: EmlMixins,
    pub children: EmlContent,
}

impl EmlNode {
    pub fn fetch_variables(&self, variables: &mut HashMap<Ident, Variable>, root: bool) -> syn::Result<()> {
        if let Some(model) = self.model.clone() {
            if variables.contains_key(&model) {
                throw!(
                    model,
                    "Model {} already defined",
                    model.to_string()
                );
            }
            variables.insert(model.clone(), if root {
                 Variable {
                    ident: parse_quote! { __root__ },
                    ty: self.tag.clone(),
                    kind: VariableKind::Model ,
                }
            } else {
                Variable {
                    ident: model.clone(),
                    ty: self.tag.clone(),
                    kind: VariableKind::Model,
                }
            });
        }
        if let EmlContent::Declared(children) = &self.children {
            for child in children.iter() {
                if let EmlChild::Node(node) = child {
                    node.fetch_variables(variables, false)?
                }
            }
        }
        Ok(())

    }

    pub fn build(&self, ctx: &EmlContext, as_root: bool) -> syn::Result<TokenStream> {
        let tag = &self.tag;
        let eml = &ctx.path("eml");
        let content = self.children.build(ctx, tag)?;
        let construct = self.args.build_construct(ctx, tag)?;
        let model = if let Some(model) = &self.model {
            quote! {{
                world.entity_mut(#model.entity).insert(#eml::IntoBundle::into_bundle(#construct));
                #model
            }}
        } else if as_root {
            quote! {{
                world.entity_mut(__root__).insert(#eml::IntoBundle::into_bundle(#construct));
                __root_model__
            }}
        } else {
            quote! {{
                let __entity__ = world.spawn(#eml::IntoBundle::into_bundle(#construct)).id();
                #eml::Model::<#tag>::new(__entity__)
            }}
        };
        let apply_mixins = self.mixins.build(ctx, &quote! { __model__.entity })?;
        let apply_extensions = self
            .args
            .build_extensions(ctx, tag, &quote! { __entity__ })?;
        Ok(quote_spanned! {self.tag.span()=> {
            let __model__ = #model;
            let __content__ = #content;
            <#tag as #eml::Element>::build_element(__content__)
                .eml()
                .write(world, __model__.entity);
            {
                let __entity__ = world.entity_mut(__model__.entity);
                #apply_extensions
            }
            #apply_mixins
            __model__
        }})
    }
}

impl Parse for EmlNode {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut tag: Ident = input.parse()?;
        let mut model: Option<Ident> = None;
        if input.peek(Token![:]) {
            model = Some(tag);
            input.parse::<Token![:]>()?;
            tag = input.parse()?;
        }
        let args = if input.peek(token::Brace) {
            EmlParams::braced(input)?
        } else {
            EmlParams::empty()
        };
        let mixins = input.parse()?;
        let children = input.parse()?;
        Ok(EmlNode {
            tag,
            model,
            args,
            mixins,
            children,
        })
    }
}

pub enum VariableKind {
    Model,
    Resource,
}

pub struct Variable {
    ident: Ident,
    ty: Ident,
    kind: VariableKind,
}

impl Variable {
    pub fn is_helper(&self) -> bool {
        &self.ident.to_string() == "DOT_AUTOCOMPLETE_TOKEN"
    }
    pub fn is_model(&self) -> bool {
        match self.kind {
            VariableKind::Model => true,
            VariableKind::Resource => false,
        }
    }
    pub fn is_resource(&self) -> bool {
        match self.kind {
            VariableKind::Model => false,
            VariableKind::Resource => true,
        }
    }
}

pub struct Eml {
    pub span: Span,
    pub strict: bool,
    pub directives: Vec<EmlDirective>,
    pub roots: Vec<EmlRoot>,
}

impl Parse for Eml {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let span = input.span();
        let mut directives = vec![];
        loop {
            let direcitve = input.parse::<EmlDirective>()?;
            if direcitve.is_none() {
                break;
            }
            directives.push(direcitve);
        }
        let mut roots = vec![];
        for root in input.parse_terminated(EmlRoot::parse, Token![;])? {
            roots.push(root);
        }
        Ok(Eml {
            directives,
            roots,
            span,
            strict: false,
        })
    }
}

impl Eml {
    pub fn fetch_variables(&self) -> syn::Result<HashMap<Ident, Variable>> {
        let mut variables = HashMap::new();
        let autocomplete = format_ident!("DOT_AUTOCOMPLETE_TOKEN");
        variables.insert(autocomplete.clone(), Variable { 
            ident: autocomplete.clone(),
            ty: autocomplete.clone(),
            kind: VariableKind::Model,
        });
        for directive in self.directives.iter() {
            if let EmlDirective::Resource(ident, ty) = directive {
                variables.insert(ident.clone(), Variable {
                    ident: ident.clone(),
                    ty: ty.clone(),
                    kind: VariableKind::Resource,
                });
            }
        }
        for root in self.roots.iter() {
            root.fetch_variables(&mut variables)?;
        }
        Ok(variables)
    }
    pub fn build(&self) -> syn::Result<TokenStream> {
        let bevy = quote! { ::bevy::prelude };
        let mut body = quote! {};
        let variables = self.fetch_variables()?;
        let mut root_ty = None;

        let ctx = EmlContext {
            variables,
            context: Context::new("polako"),
            strict: self.strict,
        };
        let eml = ctx.path("eml");
        for (ident, variable) in ctx.variables.iter().filter(|v| v.1.is_model() && !v.1.is_helper()) {
            let entity = &variable.ident;
            let tag = &variable.ty;
            if ident == entity {
                body = quote! { #body
                    let #ident = world.spawn_empty().id();
                    let #ident: #eml::Model<#tag> = #eml::Model::new(#ident);
                }
            } else {
                body = quote! { #body
                    let #ident: #eml::Model<#tag> = #eml::Model::new(#entity);
                }
            }
        }
        for directive in self.directives.iter() {
            let EmlDirective::Bind(bind) = directive else {
                continue;
            };
            let mut from_path = bind.from.path.clone();
            let from_var = from_path.remove(0);
            let Some(from_var) = ctx.variables.get(&from_var) else {
                throw!(from_var, "Undeclared variable {}", from_var.to_string());
            };

            let from_ty = &from_var.ty;
            let from_prop = Prop {
                root: parse_quote!(#from_ty),
                path: from_path,
            }.build(&ctx.context)?;
            let from_prop = if let Some(map) = &bind.from.map {
                let map = map.build(&ctx)?;
                quote! { #from_prop.map(#map) }
            } else {
                from_prop
            };
            let from_bind = if from_var.is_model() {
                let ident = &from_var.ident;
                quote! { #ident.entity.get(#from_prop) }
            } else {
                quote! { #from_prop.into() }
            };

            let mut to_path = bind.to.path.clone();
            let to_var = to_path.remove(0);
            let Some(to_var) = ctx.variables.get(&to_var) else {
                throw!(to_var, "Undeclared variable {}", to_var.to_string());
            };
            let to_ty = &to_var.ty;
            let to_prop = Prop {
                root: parse_quote!(#to_ty),
                path: to_path,
            }.build(&ctx.context)?;
            if let Some(map) = &bind.to.map {
                throw!(map, "Bind target prop can't be mapped.");
            }
            if to_var.is_resource() {
                throw!(bind.to.path[0], "Resources can't be used as bind targets.");
            }
            let to_bind = {
                let ident = &to_var.ident;
                quote! { 
                    #ident.entity.set(#to_prop)
                }
            };
            body = if from_var.is_model() {
                quote! { #body
                    world.bind_component_to_component(#from_bind, #to_bind);
                }
            } else {
                quote! { #body
                    world.bind_resource_to_component(#from_bind, #to_bind);
                }
            };
        }
        for root in self.roots.iter() {
            let build = root.build(&ctx)?;
            body = quote! {
                #body
                #build;
            };
            root_ty = Some(root.tag());
        }
        let Some(root_ty) = root_ty else {
            throw!(self.span, "Can't detect Eml exact type");
        };
        let body = quote! {
            #eml::Eml::<#root_ty>::new(move |world: &mut #bevy::World, __root__: #bevy::Entity| {
                let __this__ = __root__;
                #body
            })
        };
        Ok(if self.strict {
            quote! { #eml::Blueprint::new(#body) }
        } else {
            body
        })
    }
}
