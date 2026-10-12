//! Captions the analysis tools write into cells, in the UI language. They
//! follow the wording of Excel's Analysis ToolPak and pivot tables so the
//! output reads the same as in Office.

/// Language of the text written into output cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lang {
    #[default]
    En,
    Es,
}

impl Lang {
    /// Picks the language from a locale tag such as `es_ES.UTF-8`.
    pub fn from_tag(tag: &str) -> Lang {
        if tag.to_ascii_lowercase().starts_with("es") { Lang::Es } else { Lang::En }
    }

    /// The caption for `key`; unknown keys come back unchanged.
    pub fn text(self, key: &str) -> String {
        match TABLE.iter().find(|(k, _, _)| *k == key) {
            Some((_, en, es)) => (if self == Lang::Es { es } else { en }).to_string(),
            None => key.to_string(),
        }
    }

    /// [`Lang::text`] with `{0}`, `{1}`... replaced by `args`.
    pub fn fmt(self, key: &str, args: &[&str]) -> String {
        let mut s = self.text(key);
        for (i, a) in args.iter().enumerate() {
            s = s.replace(&format!("{{{}}}", i), a);
        }
        s
    }

    /// A percentage as the tools print it in captions: `95.0` / `95,0`.
    pub fn pct1(self, p: f64) -> String {
        let s = format!("{:.1}", p);
        if self == Lang::Es { s.replace('.', ",") } else { s }
    }

    /// Like [`Lang::pct1`] without decimals when whole (`95` in `Lower 95%`).
    pub fn pct(self, p: f64) -> String {
        if p.fract() == 0.0 { format!("{}", p as i64) } else { self.pct1(p) }
    }
}

/// (key, English, Spanish)
const TABLE: &[(&str, &str, &str)] = &[
    // Series names
    ("column_n", "Column {0}", "Columna {0}"),
    ("row_n", "Row {0}", "Fila {0}"),
    ("variable_n", "Variable {0}", "Variable {0}"),
    // Descriptive statistics
    ("mean", "Mean", "Media"),
    ("std_error", "Standard Error", "Error típico"),
    ("median", "Median", "Mediana"),
    ("mode", "Mode", "Moda"),
    ("std_dev", "Standard Deviation", "Desviación estándar"),
    ("sample_variance", "Sample Variance", "Varianza de la muestra"),
    ("kurtosis", "Kurtosis", "Curtosis"),
    ("skewness", "Skewness", "Coeficiente de asimetría"),
    ("range", "Range", "Rango"),
    ("minimum", "Minimum", "Mínimo"),
    ("maximum", "Maximum", "Máximo"),
    ("sum", "Sum", "Suma"),
    ("count", "Count", "Cuenta"),
    ("largest", "Largest({0})", "Mayor ({0})"),
    ("smallest", "Smallest({0})", "Menor({0})"),
    ("confidence", "Confidence Level({0}%)", "Nivel de confianza({0}%)"),
    // Regression
    ("summary_output", "SUMMARY OUTPUT", "Resumen"),
    ("regression_statistics", "Regression Statistics", "Estadísticas de la regresión"),
    ("multiple_r", "Multiple R", "Coeficiente de correlación múltiple"),
    ("r_square", "R Square", "Coeficiente de determinación R^2"),
    ("adjusted_r_square", "Adjusted R Square", "R^2 ajustado"),
    ("observations", "Observations", "Observaciones"),
    ("anova", "ANOVA", "ANÁLISIS DE VARIANZA"),
    ("df", "df", "Grados de libertad"),
    ("ss", "SS", "Suma de cuadrados"),
    ("ms", "MS", "Promedio de los cuadrados"),
    ("f", "F", "F"),
    ("significance_f", "Significance F", "Valor crítico de F"),
    ("regression", "Regression", "Regresión"),
    ("residual", "Residual", "Residuos"),
    ("total", "Total", "Total"),
    ("coefficients", "Coefficients", "Coeficientes"),
    ("t_stat", "t Stat", "Estadístico t"),
    ("p_value", "P-value", "Probabilidad"),
    ("lower_pct", "Lower {0}%", "Inferior {0}%"),
    ("upper_pct", "Upper {0}%", "Superior {0}%"),
    ("intercept", "Intercept", "Intercepción"),
    ("x_variable_n", "X Variable {0}", "Variable X {0}"),
    ("residual_output", "RESIDUAL OUTPUT", "Análisis de los residuales"),
    ("observation", "Observation", "Observación"),
    ("predicted_y", "Predicted {0}", "Pronóstico para {0}"),
    ("residuals", "Residuals", "Residuos"),
    ("standard_residuals", "Standard Residuals", "Residuos estándares"),
    ("y", "Y", "Y"),
    // Histogram
    ("bin", "Bin", "Clase"),
    ("frequency", "Frequency", "Frecuencia"),
    ("cumulative_pct", "Cumulative %", "% acumulado"),
    ("more", "More", "y mayor..."),
    // t-Test, F-Test, ANOVA
    ("t_paired", "t-Test: Paired Two Sample for Means", "Prueba t para medias de dos muestras emparejadas"),
    ("t_equal", "t-Test: Two-Sample Assuming Equal Variances", "Prueba t para dos muestras suponiendo varianzas iguales"),
    ("t_unequal", "t-Test: Two-Sample Assuming Unequal Variances", "Prueba t para dos muestras suponiendo varianzas desiguales"),
    ("variance", "Variance", "Varianza"),
    ("pearson", "Pearson Correlation", "Coeficiente de correlación de Pearson"),
    ("pooled_variance", "Pooled Variance", "Varianza agrupada"),
    ("hyp_mean_diff", "Hypothesized Mean Difference", "Diferencia hipotética de las medias"),
    ("p_one_tail", "P(T<=t) one-tail", "P(T<=t) una cola"),
    ("t_crit_one_tail", "t Critical one-tail", "Valor crítico de t (una cola)"),
    ("p_two_tail", "P(T<=t) two-tail", "P(T<=t) dos colas"),
    ("t_crit_two_tail", "t Critical two-tail", "Valor crítico de t (dos colas)"),
    ("f_test", "F-Test Two-Sample for Variances", "Prueba F para varianzas de dos muestras"),
    ("pf_one_tail", "P(F<=f) one-tail", "P(F<=f) una cola"),
    ("f_crit_one_tail", "F Critical one-tail", "Valor crítico para F (una cola)"),
    ("anova_single", "Anova: Single Factor", "Análisis de varianza de un factor"),
    ("summary", "SUMMARY", "RESUMEN"),
    ("groups", "Groups", "Grupos"),
    ("average", "Average", "Promedio"),
    ("source_of_variation", "Source of Variation", "Origen de las variaciones"),
    ("f_crit", "F crit", "Valor crítico para F"),
    ("between_groups", "Between Groups", "Entre grupos"),
    ("within_groups", "Within Groups", "Dentro de los grupos"),
    // Rank and percentile
    ("point", "Point", "Posición"),
    ("rank", "Rank", "Jerarquía"),
    ("percent", "Percent", "Porcentaje"),
    // Scenario summary
    ("scenario_summary", "Scenario Summary", "Resumen del escenario"),
    ("current_values", "Current Values:", "Valores actuales:"),
    ("changing_cells", "Changing Cells:", "Celdas cambiantes:"),
    ("result_cells", "Result Cells:", "Celdas de resultado:"),
    ("scenario_note", "Current Values column represents values of changing cells at time Scenario Summary Report was created.", "La columna de valores actuales representa los valores de las celdas cambiantes en el momento en que se creó el Informe resumen de escenario."),
    // Data table / goal seek sheets
    ("sheet_scenario", "Scenario Summary", "Resumen del escenario"),
    ("sheet_stats", "Analysis", "Análisis"),
    ("sheet_pivot", "PivotTable", "Tabla dinámica"),
    ("sheet_forecast", "Forecast", "Previsión"),
    // Forecast sheet
    ("fc_timeline", "Timeline", "Escala de tiempo"),
    ("fc_forecast", "Forecast({0})", "Previsión({0})"),
    ("fc_lower", "Lower Confidence Bound({0})", "Límite de confianza inferior({0})"),
    ("fc_upper", "Upper Confidence Bound({0})", "Límite de confianza superior({0})"),
    ("fc_statistic", "Statistic", "Estadística"),
    ("fc_value", "Value", "Valor"),
    // Pivot tables
    ("row_labels", "Row Labels", "Etiquetas de fila"),
    ("column_labels", "Column Labels", "Etiquetas de columna"),
    ("grand_total", "Grand Total", "Total general"),
    ("item_total", "{0} Total", "Total {0}"),
    ("values", "Values", "Valores"),
    ("all", "(All)", "(Todas)"),
    ("multiple_items", "(Multiple Items)", "(Varios elementos)"),
    ("blank", "(blank)", "(en blanco)"),
    ("sum_of", "Sum of {0}", "Suma de {0}"),
    ("count_of", "Count of {0}", "Cuenta de {0}"),
    ("average_of", "Average of {0}", "Promedio de {0}"),
    ("max_of", "Max of {0}", "Máx. de {0}"),
    ("min_of", "Min of {0}", "Mín. de {0}"),
    ("product_of", "Product of {0}", "Producto de {0}"),
    ("countnums_of", "Count of {0}", "Contar números de {0}"),
    ("stddev_of", "StdDev of {0}", "DesvEst de {0}"),
    ("stddevp_of", "StdDevp of {0}", "DesvEstP de {0}"),
    ("var_of", "Var of {0}", "Var de {0}"),
    ("varp_of", "Varp of {0}", "VarP de {0}"),
    ("quarter_n", "Qtr{0}", "Trim.{0}"),
    ("total_of", "Total {0}", "Total {0}"),
    ("distinct_of", "Distinct Count of {0}", "Recuento distinto de {0}"),
    ("sheet_details", "Details", "Detalles"),
];

/// Short month names for grouped dates (pivot tables).
pub fn month_short(lang: Lang, m: u32) -> &'static str {
    const EN: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    const ES: [&str; 12] = ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"];
    let i = (m.clamp(1, 12) - 1) as usize;
    if lang == Lang::Es { ES[i] } else { EN[i] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captions() {
        assert_eq!(Lang::En.text("mean"), "Mean");
        assert_eq!(Lang::Es.text("mean"), "Media");
        assert_eq!(Lang::Es.fmt("confidence", &[&Lang::Es.pct1(95.0)]), "Nivel de confianza(95,0%)");
        assert_eq!(Lang::En.fmt("lower_pct", &[&Lang::En.pct(95.0)]), "Lower 95%");
        assert_eq!(Lang::En.text("nope"), "nope");
        assert_eq!(Lang::from_tag("es_ES.UTF-8"), Lang::Es);
        // Keys are unique.
        for (i, (k, _, _)) in TABLE.iter().enumerate() {
            assert!(TABLE[i + 1..].iter().all(|(o, _, _)| o != k), "{k}");
        }
    }
}
