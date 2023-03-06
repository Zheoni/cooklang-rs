use std::{collections::HashMap, path::PathBuf, time::Duration};

use clap::{Args, ValueEnum};
use cooklang::{
    convert::Converter,
    model::{Component, ComponentKind, Ingredient},
    quantity::Quantity,
    scale::{ScaleOutcome, ScaleTarget},
    CooklangParser,
};
use miette::{bail, Context, IntoDiagnostic, Result};

use super::RecipeInput;

#[derive(Debug, Args)]
pub struct ReadArgs {
    #[command(flatten)]
    input: RecipeInput,

    /// Output file, none for stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Output format
    #[arg(short, long, value_enum, default_value_t)]
    format: Output,

    /// Pretty output format if available
    #[arg(long)]
    pretty: bool,

    /// Do not display warnings
    #[arg(long)]
    ignore_warnings: bool,

    #[arg(long)]
    scale: Option<u32>,
}

#[derive(Debug, Default, Clone, Copy, ValueEnum)]
enum Output {
    #[default]
    Human,
    Json,
}

pub fn run(parser: &CooklangParser, args: ReadArgs) -> Result<()> {
    let (input, name) = args.input.read()?;

    let (recipe, warnings) = parser.parse(&input, &name)?;
    if !args.ignore_warnings {
        cooklang::error::print_warnings(
            &input,
            &warnings,
            crate::STD_ERR_COLOR_ENABLED.load(std::sync::atomic::Ordering::Relaxed),
        );
    }

    let scaled_recipe = if let Some(scale) = args.scale {
        let target = if let Some(servings) = recipe.metadata.servings.as_ref() {
            let Some(base) = servings.first().copied() else { bail!("Empty servings list") };
            ScaleTarget::new(base, scale, servings)
        } else {
            ScaleTarget::new(1, scale, &[])
        };
        recipe.scale(target)
    } else if let Some(servings) = &recipe.metadata.servings {
        let Some(base) = servings.first().copied() else { bail!("Empty servings list") };
        let target = ScaleTarget::new(base, base, servings);
        recipe.scale(target)
    } else {
        recipe.skip_scaling()
    };
    args.to_output(&scaled_recipe, parser.converter())?;

    Ok(())
}

impl ReadArgs {
    fn to_output(&self, recipe: &cooklang::ScaledRecipe, converter: &Converter) -> Result<()> {
        if let Some(path) = &self.output {
            let file = std::fs::File::create(path)
                .into_diagnostic()
                .wrap_err("Failed to create output file")?;
            self.write(recipe, converter, file)?;
        } else {
            self.write(recipe, converter, std::io::stdout())?;
        };
        Ok(())
    }

    fn write(
        &self,
        recipe: &cooklang::ScaledRecipe,
        converter: &Converter,
        writer: impl std::io::Write,
    ) -> Result<()> {
        match self.format {
            Output::Human => {
                print_human(recipe, converter, writer).into_diagnostic()?;
            }
            Output::Json => {
                if self.pretty {
                    serde_json::to_writer_pretty(writer, recipe).into_diagnostic()?;
                } else {
                    serde_json::to_writer(writer, recipe).into_diagnostic()?;
                }
            }
        };
        Ok(())
    }
}

fn print_human(
    recipe: &cooklang::ScaledRecipe,
    converter: &Converter,
    mut writer: impl std::io::Write,
) -> Result<(), std::io::Error> {
    use console::style;
    use cooklang::model::Item;
    use std::fmt::Write;
    use tabular::{Row, Table};

    let total_quantities = {
        let mut v = Vec::with_capacity(recipe.ingredients.len());
        for igr in &recipe.ingredients {
            v.push(
                igr.total_quantity(&recipe.ingredients, converter)
                    .ok()
                    .flatten(),
            )
        }
        v
    };

    let termwidth = textwrap::termwidth().min(80);

    let w = &mut writer;

    let quantity_fmt = |qty: &Quantity| {
        if let Some(unit) = qty.unit() {
            format!("{} {}", qty.value, style(unit.text()).italic())
        } else {
            format!("{}", qty.value)
        }
    };

    writeln!(
        w,
        "{} {}\n",
        style(format!(
            " {}{} ",
            recipe
                .metadata
                .emoji
                .map(|e| e.to_owned() + " ")
                .unwrap_or_default(),
            recipe.name
        ))
        .bright()
        .white()
        .on_magenta()
        .bold(),
        style(
            recipe
                .metadata
                .slug
                .as_deref()
                .map(|slug| {
                    let default_slug = cooklang::metadata::slugify(&recipe.name);
                    if slug != default_slug {
                        format!("({slug})")
                    } else {
                        String::new()
                    }
                })
                .unwrap_or_default()
        )
        .dim()
    )?;

    if !recipe.metadata.tags.is_empty() {
        let mut tags = String::new();
        for tag in &recipe.metadata.tags {
            let hash = tag
                .chars()
                .enumerate()
                .map(|(i, c)| c as usize * i)
                .reduce(usize::wrapping_add)
                .map(|h| (h % 7))
                .unwrap_or_default();
            let color = match hash {
                0 => console::Color::Red,
                1 => console::Color::Blue,
                2 => console::Color::Cyan,
                3 => console::Color::Yellow,
                4 => console::Color::Green,
                5 => console::Color::Magenta,
                6 => console::Color::White,
                _ => unreachable!(),
            };
            tags += &style(format!("#{tag}")).fg(color).to_string();
            tags += " ";
        }
        let tags = textwrap::fill(&tags, termwidth);
        writeln!(w, "{tags}\n")?;
    }

    if let Some(desc) = &recipe.metadata.description {
        let desc = textwrap::fill(
            desc,
            textwrap::Options::new(termwidth)
                .initial_indent("\u{2502} ")
                .subsequent_indent("\u{2502}"),
        );
        writeln!(w, "{desc}\n")?;
    }

    let mut some_meta = false;
    let mut meta_fmt =
        |name: &str, value: &str| writeln!(w, "{}: {}", style(name).bold().green(), value);
    if let Some(author) = &recipe.metadata.author {
        some_meta = true;
        let text = author
            .name
            .as_deref()
            .or(author.url.as_ref().map(|u| u.as_str()))
            .unwrap_or("-");
        meta_fmt("author", text)?;
    }
    if let Some(source) = &recipe.metadata.source {
        some_meta = true;
        let text = source
            .name
            .as_deref()
            .or(source.url.as_ref().map(|u| u.as_str()))
            .unwrap_or("-");
        meta_fmt("source", text)?;
    }
    if let Some(time) = &recipe.metadata.time {
        some_meta = true;
        let time_fmt = |t: u32| {
            format!(
                "{}",
                humantime::format_duration(Duration::from_secs(t as u64 * 60))
            )
        };
        match time {
            cooklang::metadata::RecipeTime::Total(t) => meta_fmt("time", &time_fmt(*t))?,
            cooklang::metadata::RecipeTime::Composed {
                prep_time,
                cook_time,
            } => {
                if let Some(p) = prep_time {
                    meta_fmt("prep time", &time_fmt(*p))?
                }
                if let Some(c) = cook_time {
                    meta_fmt("cook time", &time_fmt(*c))?;
                }
            }
        }
    }
    if let Some(servings) = &recipe.metadata.servings {
        let index = recipe.scaled_data().and_then(|d| d.target.index());
        let mut text = servings
            .iter()
            .enumerate()
            .map(|(i, s)| {
                if Some(i) == index {
                    style(format!("[{s}]")).bold().yellow().bright().to_string()
                } else {
                    s.to_string()
                }
            })
            .reduce(|a, b| format!("{a}|{b}"))
            .unwrap_or_default();
        if let Some(data) = recipe.scaled_data() {
            if data.target.index().is_none() {
                write!(
                    &mut text,
                    " {} {}",
                    style("->").red(),
                    style(data.target.target_servings()).red()
                )
                .unwrap();
            }
        }
        meta_fmt("servings", &text)?;
    }
    if some_meta {
        writeln!(w)?;
    }

    if !recipe.ingredients.is_empty() {
        writeln!(w, "Ingredients:")?;
    }
    assert_eq!(total_quantities.len(), recipe.ingredients.len());
    let mut table = Table::new("  {:<}    {:<} {:<}");
    let mut there_is_fixed = false;
    let mut there_is_err = false;
    for ((index, igr), total_quantity) in recipe
        .ingredients
        .iter()
        .enumerate()
        .zip(total_quantities)
        .filter(|((_, igr), _)| !igr.is_hidden() && !igr.is_reference())
    {
        let mut is_fixed = false;
        let mut is_err = false;
        let s = recipe
            .scaled_data()
            .map(|d| {
                let mut o = &d.ingredients[index];
                if matches!(o, ScaleOutcome::Error(_)) {
                    return o;
                }
                for &r in igr.referenced_from() {
                    match (&o, &d.ingredients[r]) {
                        (_, e @ ScaleOutcome::Error(_)) => return e,
                        (_, e @ ScaleOutcome::Fixed) => o = e,
                        _ => {}
                    }
                }
                o
            })
            .map(|outcome| match outcome {
                ScaleOutcome::Fixed => {
                    there_is_fixed = true;
                    is_fixed = true;
                    console::Style::new().yellow()
                }
                ScaleOutcome::Error(_) => {
                    there_is_err = true;
                    is_err = true;
                    console::Style::new().on_red()
                }
                ScaleOutcome::Scaled | ScaleOutcome::NoQuantity => console::Style::new(),
            })
            .unwrap_or_default();
        let mut row = Row::new().with_cell(&igr.name);
        if let Some(quantity) = total_quantity {
            row.add_ansi_cell(s.apply_to(quantity_fmt(&quantity)));
        } else {
            let list = igr
                .all_quantities(&recipe.ingredients)
                .into_iter()
                .map(|q| quantity_fmt(q))
                .reduce(|s, q| format!("{s}, {q}"));
            if let Some(list) = list {
                row.add_ansi_cell(s.apply_to(list));
            } else {
                row.add_cell("");
            }
        }
        if let Some(note) = &igr.note {
            row.add_cell(note);
        } else {
            row.add_cell("");
        }
        table.add_row(row);
    }
    write!(w, "{table}")?;
    if crate::STD_ERR_COLOR_ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
        if there_is_fixed || there_is_err {
            if there_is_fixed {
                write!(w, "{} fixed value", style("\u{25A0}").yellow())?;
            }
            if there_is_err {
                if there_is_fixed {
                    write!(w, " | ")?;
                }
                write!(w, "{} error scaling", style("\u{25A0}").red())?;
            }
            writeln!(w)?;
        }
    }
    writeln!(w)?;

    writeln!(w, "Steps:")?;
    for section in &recipe.sections {
        if let Some(name) = &section.name {
            writeln!(w, "{}", style(name).underlined())?;
        }
        let mut step_counter = 0;
        for step in &section.steps {
            if step.is_text {
                write!(w, "")?;
                for item in &step.items {
                    if let Item::Text(text) = item {
                        write!(w, "{text}")?;
                    } else {
                        panic!("Not text in text step");
                    }
                }
            } else {
                step_counter += 1;
                write!(w, "{step_counter:>2}. ")?;

                let mut step_text = String::new();
                let mut step_igrs_duplicates: HashMap<&str, Vec<usize>> = HashMap::new();
                let mut step_igrs: Vec<(&Ingredient, Option<usize>)> = Vec::new();
                for item in &step.items {
                    if let Item::Component(Component {
                        kind: ComponentKind::Ingredient,
                        index,
                    }) = item
                    {
                        let igr = &recipe.ingredients[*index];
                        step_igrs_duplicates
                            .entry(&igr.name)
                            .or_default()
                            .push(*index);
                    }
                }
                for group in step_igrs_duplicates.values_mut() {
                    let first = group.first().copied().unwrap();
                    group.retain(|&i| recipe.ingredients[i].quantity.is_some());
                    if group.is_empty() {
                        group.push(first);
                    }
                }
                for item in &step.items {
                    match item {
                        Item::Text(text) => step_text += text,
                        Item::Component(c) => match c.kind {
                            ComponentKind::Ingredient => {
                                let igr = &recipe.ingredients[c.index];
                                write!(&mut step_text, "{}", style(&igr.name).green()).unwrap();
                                let pos = write_igr_count(
                                    &mut step_text,
                                    &step_igrs_duplicates,
                                    c.index,
                                    &igr.name,
                                );
                                // skip references that adds no value to the reader
                                if pos.is_none() || igr.quantity.is_some() {
                                    step_igrs.push((igr, pos));
                                }
                            }
                            ComponentKind::Cookware => {
                                let cookware = &recipe.cookware[c.index];
                                write!(&mut step_text, "{}", style(&cookware.name).yellow())
                                    .unwrap();
                            }
                            ComponentKind::Timer => {
                                let timer = &recipe.timers[c.index];
                                write!(
                                    &mut step_text,
                                    "{}",
                                    style(quantity_fmt(&timer.quantity)).cyan()
                                )
                                .unwrap();
                            }
                        },
                        Item::InlineQuantity(temp) => write!(w, "{}", temp)?,
                    }
                }
                let step_text = textwrap::fill(
                    &step_text,
                    textwrap::Options::new(termwidth).subsequent_indent("    "),
                );
                writeln!(w, "{step_text}")?;
                if step_igrs.is_empty() {
                    write!(w, "    [-]")?;
                } else {
                    let mut igrs_text = String::from("    [");
                    for (i, (igr, pos)) in step_igrs.iter().enumerate() {
                        write!(&mut igrs_text, "{}", igr.name).unwrap();
                        if let Some(pos) = pos {
                            write_subscript(&mut igrs_text, &pos.to_string());
                        }
                        if let Some(q) = &igr.quantity {
                            write!(&mut igrs_text, ": {}", style(quantity_fmt(q)).dim()).unwrap();
                        }
                        if i != step_igrs.len() - 1 {
                            igrs_text += ", ";
                        }
                    }
                    igrs_text += "]";
                    let igrs_text = textwrap::fill(
                        &igrs_text,
                        textwrap::Options::new(termwidth).subsequent_indent("     "),
                    );
                    write!(w, "{igrs_text}")?;
                }
            }
            writeln!(w)?;
        }
        writeln!(w)?
    }

    Ok(())
}

fn write_igr_count(
    buffer: &mut String,
    step_igrs: &HashMap<&str, Vec<usize>>,
    index: usize,
    name: &str,
) -> Option<usize> {
    let entries = &step_igrs[name];
    let count = entries.len();
    if count > 1 {
        let pos = entries.iter().position(|&i| i == index).unwrap() + 1;
        write_subscript(buffer, &pos.to_string());
        Some(pos)
    } else {
        None
    }
}

fn write_subscript(buffer: &mut String, s: &str) {
    buffer.reserve(s.len());
    s.chars()
        .map(|c| match c {
            '0' => '₀',
            '1' => '₁',
            '2' => '₂',
            '3' => '₃',
            '4' => '₄',
            '5' => '₅',
            '6' => '₆',
            '7' => '₇',
            '8' => '₈',
            '9' => '₉',
            _ => c,
        })
        .for_each(|c| buffer.push(c))
}
