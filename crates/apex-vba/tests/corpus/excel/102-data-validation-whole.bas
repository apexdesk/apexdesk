' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
' corpus-prints: 100
Sub Main()
    With Range("B2:B5").Validation
        .Delete
        .Add Type:=xlValidateWholeNumber, AlertStyle:=xlValidAlertStop, Operator:=xlBetween, Formula1:="1", Formula2:="100"
        .ErrorMessage = "Enter 1 to 100"
    End With
    Debug.Print Range("B2").Validation.Formula1
    Debug.Print Range("B2").Validation.Formula2
End Sub
